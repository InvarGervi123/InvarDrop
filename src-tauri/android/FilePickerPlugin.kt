package io.github.invargervi.invardrop

import android.app.Activity
import android.content.Intent
import android.provider.OpenableColumns
import android.system.Os
import android.system.OsConstants
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.PermissionState
import android.os.Build
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.atomic.AtomicBoolean

@InvokeArg
class PickDocumentsArgs {
    lateinit var directory: String
    var multiple: Boolean = false
}

@TauriPlugin(permissions = [Permission(strings = [NetworkPermissionPolicy.PERMISSION], alias = "localNetwork")])
class FilePickerPlugin(private val activity: Activity) : Plugin(activity) {
    private val busy = AtomicBoolean(false)
    private val downloads = DownloadsPublisher(activity)

    @Command
    fun networkPermission(invoke: Invoke) {
        if (!NetworkPermissionPolicy.required(Build.VERSION.SDK_INT, activity.applicationInfo.targetSdkVersion) ||
            getPermissionState("localNetwork") == PermissionState.GRANTED) {
            invoke.resolveObject(mapOf("granted" to true))
        } else {
            requestPermissionForAlias("localNetwork", invoke, "networkPermissionResult")
        }
    }
    @PermissionCallback
    fun networkPermissionResult(invoke: Invoke) {
        invoke.resolveObject(mapOf("granted" to (getPermissionState("localNetwork") == PermissionState.GRANTED)))
    }

    @Command
    fun publish(invoke: Invoke) {
        Thread({
            try {
                val location = downloads.publish(invoke.parseArgs(PublishDownloadsArgs::class.java))
                invoke.resolveObject(mapOf("location" to location))
            } catch (_: Exception) {
                invoke.reject("Saving to Downloads failed; the verified private copy is retained for recovery")
            }
        }, "InvarDrop-Downloads").start()
    }


    @Command
    fun choose(invoke: Invoke) {
        if (!busy.compareAndSet(false, true)) {
            invoke.reject("A file selection is already in progress")
            return
        }
        try {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = "*/*"
                putExtra(Intent.EXTRA_ALLOW_MULTIPLE, invoke.parseArgs(PickDocumentsArgs::class.java).multiple)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                // Prefer local providers; InvarDrop never contacts a cloud service.
                putExtra(Intent.EXTRA_LOCAL_ONLY, true)
            }
            startActivityForResult(invoke, intent, "selected")
        } catch (_: Exception) {
            busy.set(false)
            invoke.reject("Android file picker could not be opened")
        }
    }

    @ActivityCallback
    fun selected(invoke: Invoke, result: ActivityResult) {
        if (result.resultCode != Activity.RESULT_OK) {
            busy.set(false)
            invoke.resolveObject(mapOf("names" to emptyList<String>()))
            return
        }
        // Provider queries, copying and fsync must never block the Android UI.
        Thread({
            try {
                val args = invoke.parseArgs(PickDocumentsArgs::class.java)
                val directory = File(args.directory)
                val expected = File(activity.dataDir.canonicalFile, "security-v1/prepared")
                require(directory.canonicalFile == directory.absoluteFile)
                require(directory.parentFile == expected)
                require(directory.name.startsWith("documents-"))
                require(directory.isDirectory && directory.list()?.isEmpty() == true)
                val data = result.data ?: error("Missing selection")
                val clip = data.clipData
                val uris = if (clip != null) {
                    require(clip.itemCount in 1..DocumentImport.MAX_FILES)
                    (0 until clip.itemCount).map { clip.getItemAt(it).uri }
                } else listOf(data.data ?: error("Missing URI"))
                require(args.multiple || uris.size == 1)
                val names = uris.map { uri ->
                    require(uri.scheme == "content" && uri.toString().length <= 8192)
                    activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                        require(cursor.moveToFirst())
                        cursor.getString(0).also { require(DocumentImport.safeName(it)) }
                    } ?: error("Document has no display name")
                }
                require(names.map { it.lowercase(java.util.Locale.ROOT) }.toSet().size == names.size)
                var remaining = DocumentImport.MAX_BYTES
                for ((uri, name) in uris.zip(names)) {
                    // Rust owns this new private directory. O_EXCL and O_NOFOLLOW
                    // prevent collisions; the provider never chooses a path.
                    val file = File(directory, name)
                    val fd = Os.open(file.path, OsConstants.O_WRONLY or OsConstants.O_CREAT or
                        OsConstants.O_EXCL or OsConstants.O_NOFOLLOW, 0x180) // 0600
                    FileOutputStream(fd).use { output ->
                        activity.contentResolver.openInputStream(uri)?.use { input ->
                            remaining -= DocumentImport.copy(input, output, remaining) {
                                require(directory.usableSpace >= 64L * 1024 * 1024) { "Insufficient private storage" }
                            }
                        } ?: error("Document could not be opened")
                        output.fd.sync()
                    }
                }
                invoke.resolveObject(mapOf("names" to names))
            } catch (_: Exception) {
                // Rust's TempDir removes the entire incomplete import on error.
                // Do not expose provider exceptions containing document URIs.
                invoke.reject("Selected files could not be prepared; check names, free storage and the 4 GiB import limit")
            } finally {
                busy.set(false)
            }
        }, "InvarDrop-SAF").start()
    }
}
