package io.github.invargervi.invardrop

import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest

// Platform-independent publication boundary, shared by MediaStore and JVM tests.
internal object VerifiedDownload {
    interface Row {
        val pending: Boolean
        fun read(): InputStream
        fun write(action: (OutputStream) -> Unit)
        fun commit()
        fun abandon()
    }
    fun check(input: InputStream, size: Long, sha256: String, output: OutputStream? = null) {
        require(size >= 0 && sha256.matches(Regex("[0-9a-f]{64}")))
        val digest = MessageDigest.getInstance("SHA-256")
        val buffer = ByteArray(64 * 1024)
        var total = 0L
        input.use {
            while (true) {
                val count = it.read(buffer)
                if (count == -1) break
                require(count > 0 && count.toLong() <= size - total) { "Invalid download size" }
                digest.update(buffer, 0, count)
                output?.write(buffer, 0, count)
                total += count
            }
        }
        require(total == size && digest.digest().joinToString("") { "%02x".format(it) } == sha256) {
            "Download integrity verification failed"
        }
    }
    fun publish(source: () -> InputStream, row: Row, size: Long, sha256: String) {
        // Existing public files are never opened for writing or deleted, even
        // when changed by the user. Reconnection can only reuse matching bytes.
        if (!row.pending) { check(row.read(), size, sha256); return }
        try {
            row.write { output -> check(source(), size, sha256, output) }
            check(row.read(), size, sha256)
            row.commit()
        } catch (error: Exception) {
            row.abandon() // Implementation may remove only an owned pending row.
            throw error
        }
    }
}
