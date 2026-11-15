# Milestone 2.8 — Optional File Processing

## Operation boundaries

TLS 1.3 transfer, BLAKE3, pairing, trust, checkpoints, and the network protocol are
unchanged. Processing is a separate stage requiring an explicit user decision.
When OGG candidates are found after native file/folder selection, the UI offers
**Keep originals** (the default) or **Convert OGG to MP3**. Neither sending nor
conversion starts without a decision. Without OGG candidates, the existing file
workflow continues immediately. Conversion errors never cause automatic fallback:
originals remain available and can be selected again.

The receiver approves the actual prepared names, format, and size: the manifest/
offer is built only after all preparation finishes. No file from an incomplete
conversion is transferred. Folders retain their relative structure; OGG files get
the `.mp3` extension, and name collisions fail preparation. Other files are copied
without content changes. Originals are never opened for writing.

## FFmpeg and installation

FFmpeg is **not included** in the InvarDrop executable/CI artifact, downloaded by
the application, or discovered automatically in PATH. For conversion, the user
selects a trusted local executable through **Choose FFmpeg executable**. On Windows,
this must be `ffmpeg.exe`, not BAT/CMD or a shim/launcher that starts another process.
The selection lasts until the application closes. On Linux, install the ffmpeg
package through the distribution and select `/usr/bin/ffmpeg`. Windows build options
are linked from the [official download page](https://ffmpeg.org/download.html).
The build must support OGG/Vorbis, Opus, and the `libmp3lame` encoder. Missing
encoders/decoders produce an explicit error and never send a corrupted result.

FFmpeg is distributed under LGPL 2.1+; enabling some components changes the terms
to GPL. Separate redistribution must retain the specific build's terms, library
licenses, and corresponding source code. InvarDrop neither links to FFmpeg nor
ships its binaries. See
[FFmpeg License and Legal Considerations](https://ffmpeg.org/legal.html).
Third-party builds using `--enable-nonfree` must not be assumed redistributable;
the application makes no legal claims about an arbitrary user-selected executable.
Use a current, patched build.

CI installs ffmpeg separately for real tests through Ubuntu apt and Windows
Chocolatey. This test tool is not copied into application artifacts. Local Rust
tests also require ffmpeg in PATH or an explicit
`INVARDROP_TEST_FFMPEG=/absolute/path/to/ffmpeg`. This variable is test-only;
production configuration always uses native selection.

Supported inputs are `.ogg`, `.oga`, and `.opus` with OggS/BOS and an initial
Vorbis/Opus packet. This is preliminary recognition; FFmpeg performs full parsing.
Output is MP3 using `libmp3lame`, 192 kbit/s, the first audio stream, no video, and
no automatic copying of global metadata. Conversion is **lossy** and cannot restore
the source codec's lost quality. Afterwards, the MP3 is fully decoded to a null sink
with error checking; the source is hashed in a streaming pass before/after
processing to detect changes during reading. This is not a filesystem snapshot or
protection against local-account compromise.

FFmpeg runs without a shell or stdin, with explicit arguments and no network input
protocols (`file` whitelist). Errors are displayed without dumping source data.
Cancel terminates the child and waits for it to stop. Each run is limited to two
hours, each MP3 to less than 1 GiB, and the entire prepared folder to 2 GiB.
The intermediate directory is private and is removed on error/cancellation until
all preparation completes. Conversion progress counts completed OGG files; the
current file uses an indeterminate indicator without an invented percentage. The
UI refreshes state at most twice per second.

## Resuming prepared files

Prepared copies are stored in `<profile>/prepared/audio-*/...`. Resume references
these immutable MP3 files/copies instead of converting OGG again. Resuming an
already prepared transfer does not require FFmpeg. Sources and checkpoints are
revalidated as in 2.7; prepared bytes remain unchanged during recovery.

A profile permits up to eight prepared batches. Before a new conversion, only
batches no longer referenced by recovery records are removed; active source leases
are also considered. Cleanup is deferred while an outgoing transfer is active,
protecting legacy transfers without recovery records too. Failure to validate
references prevents cleanup rather than authorizing deletion. If every batch is
needed for resume, complete or Discard the corresponding older transfers.
**Dismiss** removes only the UI row, never sources or prepared data needed for resume.

Incomplete conversion itself is not resumable: after a crash, start it again with
fresh consent. On normal exit, the application signals cancellation and allows
workers up to three seconds to finish. Forced termination or stalled I/O may leave
remnants or a child process running until it finishes; these are never considered
prepared/successful transfers. After restart, unreferenced temporary batches are
cleaned up before the next conversion.

## ZIP extraction after receiving

Only a successfully completed **incoming single-file ZIP transfer** offers a
separate checkbox and **Extract ZIP**. Keeping the archive unchanged is the default.
Extraction never starts on Accept transfer, is not enabled by a global setting,
and never executes/opens extracted files. There is no automatic recursive
extraction of nested archives, scanning of ZIP files inside received folders, or
browser for older receipts. After application restart, the saved ZIP remains on
disk, but processing history/the extraction offer is not restored.

Parsing uses Rust `zip` with unnecessary formats disabled. Ordinary ZIP with
Stored/Deflate and UTF-8 names is supported. ZIP64, split/encrypted archives, and
other compression methods are unsupported. EOCD, central-directory limits, entry
counts/sizes, paths, and modes are checked before staging is created.

| Limit | Value |
| --- | --- |
| Source ZIP | 64 MiB |
| Central directory | 1 MiB |
| Files and directories, including implicit parents | 4096 |
| Depth | 16 components |
| Component / relative path | 200 / 1024 UTF-8 bytes |
| Single extracted file | 512 MiB |
| Total extracted data | 1 GiB |
| Expansion ratio | Up to 100:1, with a 1 MiB allowance for a small entry |

`.`/`..`, absolute paths, backslashes, Windows drive/ADS/device aliases, control/
bidi characters, and unsafe endings are forbidden. Symlinks, special files,
inconsistent types, duplicates, and case-insensitive collisions are rejected.
Limits are checked against both metadata and actual bytes read. CRC is checked
on full reads; size/CRC violations stop the entire operation.

Contents are written in 64 KiB chunks using `create_new` into a private `.extract-*`
directory beside the receipt. The incomplete tree is never published. Once all
entries pass validation, it is renamed with no replacement into a new
`extracted-*/files` directory; existing files/folders are never replaced. Until
publication begins, Cancel removes staging; afterwards, cancellation can no longer
win. The original ZIP is preserved after success, error, or cancellation.

Extracted files receive neither executable bits nor archive ACLs. This does not
make their contents safe for subsequent manual execution: the application simply
never executes them. Unix directory sync applies to staging; Windows uses its own
filesystem operations. Power-loss and unusual network-filesystem guarantees are
limited by OS contracts. After a crash, extraction starts again with separate
consent; it is not part of the network checkpoint.

## Validation

Rust tests perform real Vorbis/Opus-to-MP3 conversion, PCM decoding, duration/RMS
checks, source-preservation checks, FFmpeg failure handling, and cancellation of a
running child. Coverage includes nested/empty ZIP entries, traversal, symlinks,
collisions, CRC, bombs, entry/depth limits, cancellation during copying/finalization,
and no-replace collisions. A real TLS test transfers a prepared folder with
multiple MP3 files, interrupts it at a checkpoint, restarts the service, and sends
the remainder without converting again. Existing pairing/transfer/resume tests
are preserved. UI tests check that conversion/extraction never starts silently,
Keep originals is the default, duplicate actions are prevented, ZIP consent is
separate, and extraction is prohibited until the transfer is verified.

Windows GUI, interactive executable selection, and two physical devices require
separate manual tests; CI checks native builds, filesystem behavior, and FFmpeg tests.

## Local validation results — 2026-11-15

CachyOS/Niri: **135 Rust tests**, **49 UI tests**, no failures/ignored tests;
Rustfmt, Clippy `--all-targets -- -D warnings`, TypeScript, and production Vite/Tauri
builds using `--no-bundle -- --locked` passed. Tests verified real FFmpeg conversion,
output decoding, cancellation of a running child, unchanged originals, and prepared
MP3 retention until its resume record completes/is discarded.

The production Linux GUI and Rust probe used real keyring, discovery, and TLS.
Checks covered Extract disabled until separate consent, successful extraction of
nested and empty files, unchanged ZIP contents, and no staging after completion.
A second correctly transferred ZIP containing `../outside` was rejected through
the GUI: the original archive remained, with no output/partial files or writes
outside the destination. The GUI exited with code 0; test keyring credentials were
removed. Native GTK file/executable pickers were not automated: the Rust probe
provided the sender, and extraction consent was exercised through the GUI.
CI results apply to the specific commit on feature/optional-file-processing.
