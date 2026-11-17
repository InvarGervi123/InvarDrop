package io.github.invargervi.invardrop

import android.app.Activity
import android.content.ContentUris
import android.content.ContentValues
import android.net.Uri
import android.os.Build
import android.os.ParcelFileDescriptor
import android.webkit.MimeTypeMap
import android.os.Environment
import android.provider.MediaStore
import app.tauri.annotation.InvokeArg
import java.io.File
import java.io.FileInputStream
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest
import java.util.UUID

@InvokeArg
class DownloadEntry {
    lateinit var path: String
    var directory: Boolean = false
    var size: Long = 0
    lateinit var sha256: String
}
@InvokeArg
class PublishDownloadsArgs {
    lateinit var id: String
    lateinit var source: String
    lateinit var name: String
    var folder: Boolean = false
    var entries: List<DownloadEntry> = emptyList()
}

internal class DownloadsPublisher(private val activity: Activity) {
    private val resolver = activity.contentResolver
    private val journal = activity.getSharedPreferences("downloads-v1", Activity.MODE_PRIVATE)
    private val lock = Any()

    fun publish(args: PublishDownloadsArgs): String = synchronized(lock) {
        require(Build.VERSION.SDK_INT >= 29) { "Saving to Downloads requires Android 10 or newer" }
        require(UUID.fromString(args.id).toString() == args.id)
        require(DocumentImport.safeName(args.name))
        require(args.entries.size <= 4096)
        require(args.folder || (args.entries.size == 1 && !args.entries[0].directory && args.entries[0].path == args.name))
        val source = File(args.source)
        val inbox = File(activity.dataDir.canonicalFile, "security-v1/received")
        require(source.canonicalFile == source.absoluteFile)
        require(source.parentFile?.parentFile == inbox && source.parentFile!!.name.startsWith("received-"))
        require(if (args.folder) source.isDirectory else source.isFile)
        val receipt = "received-${args.id}"
        val relativeRoot = "${Environment.DIRECTORY_DOWNLOADS}/InvarDrop/$receipt/"
        val files = ArrayList<Pair<DownloadEntry, File>>()
        val directories = ArrayList<String>()
        val seen = HashSet<String>()
        var total = 0L
        for (entry in args.entries) {
            require(entry.path.length <= 1024 && entry.path.split('/').size <= 16)
            require(entry.path.split('/').all(DocumentImport::safeName))
            require(seen.add(entry.path.lowercase(java.util.Locale.ROOT)))
            require(entry.size in 0..(1024L * 1024 * 1024 * 1024 - total))
            total += entry.size
            val file = if (args.folder) File(source, entry.path) else source
            require(file.canonicalFile == file.absoluteFile)
            if (entry.directory) {
                require(args.folder && entry.size == 0L && file.isDirectory)
                directories.add("${args.name}/${entry.path}")
            } else {
                require(file.isFile && file.length() == entry.size && entry.sha256.matches(Regex("[0-9a-f]{64}")))
                files.add(entry to file)
            }
        }
        // Directory-only operations carry no incomplete bytes. Android's public
        // storage policy still applies; denied mkdir fails without broad permissions.
        val publicDownloads = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS).canonicalFile
        val publicRoot = File(publicDownloads, "InvarDrop/$receipt")
        if (args.folder) directories.add(0, args.name)
        for (relative in directories) {
            val directory = File(publicRoot, relative)
            require(directory.canonicalFile == directory.absoluteFile)
            require(directory.mkdirs() || directory.isDirectory)
        }
        var location = "Downloads/InvarDrop/$receipt/${args.name}"
        for ((entry, file) in files) {
            val relative = if (args.folder) "${args.name}/${entry.path}" else entry.path
            val parent = relative.substringBeforeLast('/', "")
            val relativePath = relativeRoot + if (parent.isEmpty()) "" else "$parent/"
            val key = args.id + ":" + MessageDigest.getInstance("SHA-256").digest(entry.path.toByteArray(Charsets.UTF_8))
                .joinToString("") { "%02x".format(it) }
            val uri = existing(key) ?: insert(relativePath, file.name, key)
            val row = MediaRow(uri, key, relativePath)
            VerifiedDownload.publish({ FileInputStream(file) }, row, entry.size, entry.sha256)
            if (!args.folder) location = "Downloads/InvarDrop/$receipt/${row.displayName()}"
        }
        location
    }
    private fun existing(key: String): Uri? {
        val text = journal.getString(key, null) ?: return null
        val uri = Uri.parse(text)
        require(uri.scheme == "content" && uri.authority == MediaStore.AUTHORITY)
        require(ContentUris.parseId(uri) >= 0)
        val exists = resolver.query(uri, arrayOf(MediaStore.MediaColumns._ID), null, null, null)?.use { it.moveToFirst() } == true
        if (!exists) {
            require(journal.edit().remove(key).commit())
            return null
        }
        return uri
    }
    private fun insert(relative: String, name: String, key: String): Uri {
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, MimeTypeMap.getSingleton().getMimeTypeFromExtension(
                name.substringAfterLast('.', "").lowercase(java.util.Locale.ROOT)) ?: "application/octet-stream")
            put(MediaStore.MediaColumns.RELATIVE_PATH, relative)
            put(MediaStore.MediaColumns.IS_PENDING, 1)
            put(MediaStore.MediaColumns.DATE_EXPIRES, System.currentTimeMillis() / 1000 + 24 * 60 * 60)
        }
        // MediaStore allocates a unique filename atomically; existing rows/files
        // are never opened by name for writing.
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: error("Downloads insert failed")
        if (!journal.edit().putString(key, uri.toString()).commit()) {
            resolver.delete(uri, null, null)
            error("Downloads recovery journal could not be saved")
        }
        return uri
    }
    private inner class MediaRow(private val uri: Uri, private val key: String, relative: String) : VerifiedDownload.Row {
        init {
            resolver.query(uri, arrayOf(MediaStore.MediaColumns.RELATIVE_PATH), null, null, null)?.use {
                require(it.moveToFirst() && it.getString(0) == relative)
            } ?: error("Downloads row missing")
        }
        override val pending: Boolean
            get() = resolver.query(uri, arrayOf(MediaStore.MediaColumns.IS_PENDING), null, null, null)?.use {
                require(it.moveToFirst()); it.getInt(0) == 1
            } ?: error("Downloads row missing")
        fun displayName(): String = resolver.query(uri, arrayOf(MediaStore.MediaColumns.DISPLAY_NAME), null, null, null)?.use {
            require(it.moveToFirst()); it.getString(0)
        } ?: error("Downloads row missing")
        override fun read(): InputStream = resolver.openInputStream(uri) ?: error("Downloads read failed")
        override fun write(action: (OutputStream) -> Unit) {
            require(pending)
            val descriptor = resolver.openFileDescriptor(uri, "rwt") ?: error("Downloads write failed")
            ParcelFileDescriptor.AutoCloseOutputStream(descriptor).use { output ->
                action(output)
                output.fd.sync()
            }
        }
        override fun commit() {
            val values = ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0); putNull(MediaStore.MediaColumns.DATE_EXPIRES) }
            require(resolver.update(uri, values, null, null) == 1)
        }
        override fun abandon() {
            // An uncertain commit must not delete an already published file.
            if (pending) {
                require(resolver.delete(uri, null, null) == 1)
                require(journal.edit().remove(key).commit())
            }
        }
    }
}
