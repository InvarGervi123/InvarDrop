package io.github.invargervi.invardrop

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest

private class FakeRow(var bytes: ByteArray = byteArrayOf(), override var pending: Boolean = true) : VerifiedDownload.Row {
    var writes = 0
    var commits = 0
    var abandons = 0
    var corruptReadback = false
    var failCommit = false
    override fun read(): InputStream = ByteArrayInputStream(if (corruptReadback) byteArrayOf(99) else bytes)
    override fun write(action: (OutputStream) -> Unit) {
        writes++
        val output = ByteArrayOutputStream()
        action(output)
        bytes = output.toByteArray()
    }
    override fun commit() { if (failCommit) error("Provider commit failed"); commits++; pending = false }
    override fun abandon() { abandons++; check(pending); bytes = byteArrayOf() }
}
fun main() {
    var passed = 0
    fun test(name: String, body: () -> Unit) { body(); passed++; println("PASS: $name") }
    fun fails(body: () -> Unit) { var failed = false; try { body() } catch (_: Exception) { failed = true }; check(failed) }
    fun sha(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
    val bytes = ByteArray(1200 * 1024) { (it % 251).toByte() }
    test("only a complete verified copy is made public") {
        val row = FakeRow()
        VerifiedDownload.publish({ ByteArrayInputStream(bytes) }, row, bytes.size.toLong(), sha(bytes))
        check(!row.pending && row.commits == 1 && row.bytes.contentEquals(bytes))
    }
    test("corrupted source is removed while pending") {
        val row = FakeRow()
        fails { VerifiedDownload.publish({ ByteArrayInputStream(byteArrayOf(3)) }, row, 1, sha(byteArrayOf(4))) }
        check(row.pending && row.commits == 0 && row.abandons == 1)
    }
    test("truncated or oversized data is never published") {
        for (size in listOf(0L, 2L, 4L * 1024 * 1024 * 1024 + 1)) {
            val row = FakeRow()
            fails { VerifiedDownload.publish({ ByteArrayInputStream(byteArrayOf(1)) }, row, size, sha(byteArrayOf(1))) }
            check(row.commits == 0 && row.abandons == 1)
        }
    }
    test("corrupted destination readback is never published") {
        val row = FakeRow().apply { corruptReadback = true }
        fails { VerifiedDownload.publish({ ByteArrayInputStream(bytes) }, row, bytes.size.toLong(), sha(bytes)) }
        check(row.commits == 0 && row.abandons == 1)
    }
    test("restart after publication verifies without rewriting") {
        val row = FakeRow(bytes, false)
        VerifiedDownload.publish({ error("Do not reopen the source") }, row, bytes.size.toLong(), sha(bytes))
        check(row.writes == 0 && row.commits == 0 && row.abandons == 0)
    }
    test("an edited public file is never overwritten or deleted") {
        val row = FakeRow(byteArrayOf(2), false)
        fails { VerifiedDownload.publish({ ByteArrayInputStream(byteArrayOf(1)) }, row, 1, sha(byteArrayOf(1))) }
        check(row.writes == 0 && row.abandons == 0 && row.bytes.contentEquals(byteArrayOf(2)))
    }
    test("an interrupted pending row can be retried from the private source") {
        val row = FakeRow(byteArrayOf(9))
        VerifiedDownload.publish({ ByteArrayInputStream(bytes) }, row, bytes.size.toLong(), sha(bytes))
        check(!row.pending && row.bytes.contentEquals(bytes))
    }
    test("zero-byte files still pass verification before publication") {
        val row = FakeRow()
        VerifiedDownload.publish({ ByteArrayInputStream(byteArrayOf()) }, row, 0, sha(byteArrayOf()))
        check(row.commits == 1 && row.bytes.isEmpty())
    }
    test("a failed commit does not report completion") {
        val row = FakeRow().apply { failCommit = true }
        fails { VerifiedDownload.publish({ ByteArrayInputStream(bytes) }, row, bytes.size.toLong(), sha(bytes)) }
        check(row.commits == 0 && row.abandons == 1)
    }
    println("$passed Android Downloads JVM tests passed")
}
