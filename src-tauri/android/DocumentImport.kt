package io.github.invargervi.invardrop

import java.io.InputStream
import java.io.OutputStream

// Pure bounded-copy helpers also exercised by local JVM tests.
internal object DocumentImport {
    const val MAX_FILES = 32
    const val MAX_BYTES = 4L * 1024 * 1024 * 1024
    fun safeName(name: String): Boolean = name.isNotEmpty() && name.toByteArray(Charsets.UTF_8).size <= 200 &&
        name != "." && name != ".." && !name.endsWith('.') && !name.endsWith(' ') &&
        name.none { it.code < 32 || it.code == 127 || it in "/\\:*?\"<>|" }

    fun copy(input: InputStream, output: OutputStream, limit: Long, checkSpace: () -> Unit): Long {
        require(limit >= 0)
        val buffer = ByteArray(64 * 1024)
        var total = 0L
        while (true) {
            val count = input.read(buffer)
            if (count == -1) return total
            require(count > 0) { "Document provider made no progress" }
            require(count.toLong() <= limit - total) { "Document import limit exceeded" }
            checkSpace()
            output.write(buffer, 0, count)
            total += count
        }
    }
}
