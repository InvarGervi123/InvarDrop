package io.github.invargervi.invardrop

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream

// Standalone JVM regression tests: no emulator, Android stub execution or new dependencies.
fun main() {
    var passed = 0
    fun test(name: String, body: () -> Unit) {
        body()
        passed++
        println("PASS: $name")
    }
    fun fails(body: () -> Unit) {
        var failed = false
        try { body() } catch (_: Exception) { failed = true }
        check(failed)
    }
    test("binary data is copied in bounded chunks") {
        val bytes = ByteArray(1200 * 1024) { (it % 251).toByte() }
        var largest = 0
        val input = object : ByteArrayInputStream(bytes) {
            override fun read(b: ByteArray, off: Int, len: Int): Int {
                largest = maxOf(largest, len)
                return super.read(b, off, len)
            }
        }
        val out = ByteArrayOutputStream()
        check(DocumentImport.copy(input, out, bytes.size.toLong()) {} == bytes.size.toLong())
        check(out.toByteArray().contentEquals(bytes))
        check(largest <= 64 * 1024)
    }
    test("zero-byte documents are valid") {
        check(DocumentImport.copy(ByteArrayInputStream(byteArrayOf()), ByteArrayOutputStream(), 0) {} == 0L)
    }
    test("unknown provider size cannot bypass the actual byte limit") {
        val out = ByteArrayOutputStream()
        fails { DocumentImport.copy(ByteArrayInputStream(ByteArray(11)), out, 10) {} }
        check(out.size() == 0)
    }
    test("partial reads preserve exact bytes") {
        val input = object : ByteArrayInputStream(byteArrayOf(1, 2, 3, 4)) {
            override fun read(b: ByteArray, off: Int, len: Int) = super.read(b, off, minOf(len, 1))
        }
        val out = ByteArrayOutputStream()
        check(DocumentImport.copy(input, out, 4) {} == 4L)
        check(out.toByteArray().contentEquals(byteArrayOf(1, 2, 3, 4)))
    }
    test("provider failures propagate without success") {
        fails { DocumentImport.copy(object : InputStream() {
            override fun read(): Int = throw IOException("Provider disconnected")
        }, ByteArrayOutputStream(), 100) {} }
    }
    test("low storage fails before writing the current chunk") {
        val out = ByteArrayOutputStream()
        fails { DocumentImport.copy(ByteArrayInputStream(byteArrayOf(1)), out, 1) { error("Low space") } }
        check(out.size() == 0)
    }
    test("a stalled zero-read provider cannot spin forever") {
        fails { DocumentImport.copy(object : InputStream() {
            override fun read(): Int = 0
            override fun read(b: ByteArray, off: Int, len: Int): Int = 0
        }, ByteArrayOutputStream(), 100) {} }
    }
    test("document names cannot escape the import directory") {
        for (name in listOf("..", ".", "../image.png", "/image.png", "a\\b", "a:b", "", "x".repeat(201))) {
            check(!DocumentImport.safeName(name))
        }
        check(DocumentImport.safeName("photo.png"))
        check(DocumentImport.safeName("\u043f\u0440\u0438\u043c\u0435\u0440.png"))
    }
    println("$passed Android import JVM tests passed")
}
