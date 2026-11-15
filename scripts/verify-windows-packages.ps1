param([switch]$SelfTest)
$ErrorActionPreference = 'Stop'
function Get-PackageFileHash([string]$FilePath) {
    $result = Get-FileHash -LiteralPath $FilePath -Algorithm SHA256
    if (-not $result -or -not $result.Hash) { throw "Could not hash package file: $FilePath" }
    return $result.Hash.ToLowerInvariant()
}
if ($SelfTest) {
    $temporary = Join-Path ([IO.Path]::GetTempPath()) ('invardrop-hash-[payload]-' + [guid]::NewGuid() + '.bin')
    try {
        $bytes = [byte[]](1, 2, 3, 4, 5)
        [IO.File]::WriteAllBytes($temporary, $bytes)
        $expected = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
        if ((Get-PackageFileHash $temporary) -ne $expected) { throw 'Literal archive filename hash regression failed' }
        Write-Output 'PASS: literal-path hashing of archive filenames containing square brackets'
    } finally { Remove-Item -LiteralPath $temporary -Force }
    return
}
$root = Split-Path $PSScriptRoot -Parent
$bundle = Join-Path $root 'src-tauri/target/release/bundle'
$packages = @(Get-ChildItem "$bundle/nsis/*.exe") + @(Get-ChildItem "$bundle/msi/*.msi")
if ($packages.Count -ne 2) { throw 'Expected one NSIS and one MSI installer' }
$expectedVersion = [version]((Get-Content (Join-Path $root 'src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json).version)
$report = [ordered]@{}
$auditTempBase = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
function Read-MsiRows($Database, [string]$Query, [int]$Columns) {
    $view = $Database.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $Database, @($Query))
    $view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null) | Out-Null
    while ($record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)) {
        $row = @()
        for ($index = 1; $index -le $Columns; $index++) {
            $row += $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, @($index))
        }
        ,$row
    }
    $view.GetType().InvokeMember('Close', 'InvokeMethod', $null, $view, $null) | Out-Null
}
foreach ($package in $packages) {
    $temporary = Join-Path $auditTempBase ('invardrop-package-audit-' + [guid]::NewGuid())
    New-Item -ItemType Directory -Path $temporary | Out-Null
    try {
        & 7z x $package.FullName "-o$temporary" -y | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Failed to inspect $($package.Name)" }
        $files = @(Get-ChildItem $temporary -Recurse -File)
        $inventory = [ordered]@{}
        foreach ($file in $files) {
            $relative = [IO.Path]::GetRelativePath($temporary, $file.FullName)
            if ($relative -match '(?i)(^|[\\/])(?:identity\.enc|profile\.lock|resume-v1|received|prepared|node_modules|src-tauri|\.git|\.aws|\.ssh|\.codex|\.env(?:\..*)?|ffmpeg(?:\.exe)?)([\\/]|$)') { throw "Unexpected package file: $relative" }
            $inventory[$relative] = (Get-PackageFileHash $file.FullName)
        }
        $runtime = @($files | Where-Object { $_.Name -like '*WebView2RuntimeInstaller*.exe' })
        if ($runtime.Count -ne 1) { throw 'Missing or ambiguous offline WebView2 installer' }
        $signature = Get-AuthenticodeSignature -LiteralPath $runtime[0].FullName
        if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'Microsoft Corporation') { throw 'WebView2 redistributable signature validation failed' }
        $details = [ordered]@{ sha256 = (Get-PackageFileHash $package.FullName); size = $package.Length; files = $inventory; webviewSignature = 'Valid Microsoft signature' }
        if ($package.Extension -eq '.msi') {
            $installer = New-Object -ComObject WindowsInstaller.Installer
            $database = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @($package.FullName, 0))
            $properties = @{}
            Read-MsiRows $database 'SELECT `Property`, `Value` FROM `Property`' 2 | ForEach-Object { $properties[$_[0]] = $_[1] }
            if ($properties.ProductName -ne 'InvarDrop' -or ([version]$properties.ProductVersion).ToString(3) -ne $expectedVersion.ToString(3) -or $properties.UpgradeCode.Trim('{}').ToLowerInvariant() -ne 'f74d5dcd-8cbd-51e8-8063-b651b39e44ef') { throw 'Unexpected MSI identity or upgrade code' }
            $payload = @(Read-MsiRows $database 'SELECT `FileName`, `FileSize` FROM `File`' 2)
            foreach ($entry in $payload) {
                $name = ($entry[0] -split '\|')[-1]
                if ($name -notin @('invardrop.exe', 'LICENSE', 'PACKAGING.md')) { throw "Unexpected MSI application payload: $name" }
            }
            if (-not ($payload | Where-Object { $_[0] -match 'invardrop\.exe$' })) { throw 'Missing MSI executable' }
            $actions = @(Read-MsiRows $database 'SELECT `Action`, `Source`, `Target` FROM `CustomAction`' 3)
            foreach ($action in $actions) {
                if (($action -join ' ') -match '(?i)identity\.enc|security-v1|resume-v1|APPDATA.*(?:remove|delete|rmdir)') { throw 'Unexpected MSI profile-cleanup action' }
            }
            $details.msiCustomActions = $actions
            $details.msiProperties = $properties
            $details.msiFiles = $payload
        } else {
            if (-not ($files | Where-Object { $_.Name -eq 'invardrop.exe' })) { throw 'Missing NSIS executable' }
            if (-not ($files | Where-Object { $_.Name -eq 'LICENSE' }) -or -not ($files | Where-Object { $_.Name -eq 'PACKAGING.md' })) { throw 'Missing package license or instructions' }
            $template = Get-Content (Join-Path $root 'src-tauri/target/packaging/installer.nsi') -Raw
            if ($template -match 'DeleteAppDataCheckbox|RmDir[^\r\n]*\$(?:APPDATA|LOCALAPPDATA)') { throw 'Unsafe NSIS user-data deletion found' }
        }
        $report[$package.Name] = $details
    } finally { Remove-Item -LiteralPath $temporary -Recurse -Force }
}
$report | ConvertTo-Json -Depth 12 | Set-Content "$bundle/package-audit-windows.json" -Encoding utf8
($report.GetEnumerator() | ForEach-Object { "$($_.Value.sha256)  $($_.Key)" }) | Set-Content "$bundle/SHA256SUMS-windows.txt" -Encoding utf8
Write-Output 'PASS: NSIS/MSI payload, WebView2 signatures, metadata and profile-preservation inspection'
