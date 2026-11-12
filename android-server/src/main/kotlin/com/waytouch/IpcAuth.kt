package com.waytouch

import java.io.ByteArrayOutputStream
import java.io.EOFException
import java.io.InputStream
import java.net.Socket
import java.security.MessageDigest
import java.io.File

internal fun readBoundedLine(input: InputStream, maxBytes: Int): String? {
    require(maxBytes > 0) { "maxBytes must be positive" }
    val bytes = ByteArrayOutputStream(minOf(maxBytes, 256))
    var consumed = 0
    while (true) {
        val value = input.read()
        if (value < 0) {
            if (consumed == 0) return null
            throw EOFException("connection closed before line terminator")
        }
        if (value == 10) return bytes.toString(Charsets.UTF_8.name())
        consumed += 1
        if (consumed > maxBytes) throw IllegalArgumentException("request line exceeds $maxBytes bytes")
        if (value != 13) bytes.write(value)
    }
}

internal object IpcAuth {
    private const val TOKEN_FILE = "/data/local/tmp/waytouch-private/waytouch-ipc.token"
    private const val TOKEN_LENGTH = 64

    private data class CachedToken(
        val value: String,
        val modified: Long,
        val length: Long
    )

    private val tokenLock = Any()
    @Volatile private var cachedToken: CachedToken? = null

    private fun expectedToken(forceReload: Boolean = false): String = synchronized(tokenLock) {
        val file = File(TOKEN_FILE)
        require(file.isFile && file.canRead()) { "WayTouch IPC token is missing or unreadable" }
        val modified = file.lastModified()
        val length = file.length()
        if (!forceReload) {
            cachedToken?.let { cached ->
                if (cached.modified == modified && cached.length == length) return@synchronized cached.value
            }
        }
        val token = file.readText(Charsets.US_ASCII).trim().lowercase()
        require(token.length == TOKEN_LENGTH && token.all { it in '0'..'9' || it in 'a'..'f' }) {
            "WayTouch IPC token is missing or malformed"
        }
        cachedToken = CachedToken(token, modified, length)
        token
    }

    fun authenticate(socket: Socket): Boolean {
        socket.soTimeout = 750
        val input = socket.getInputStream()
        val supplied = readBoundedLine(input, 80)
        var expected = expectedToken()
        val candidate = supplied?.removePrefix("AUTH ") ?: ""
        val validPrefix = supplied?.startsWith("AUTH ") == true
        var valid = validPrefix && MessageDigest.isEqual(
            candidate.toByteArray(Charsets.US_ASCII),
            expected.toByteArray(Charsets.US_ASCII)
        )
        if (!valid && validPrefix) {
            // A host token can rotate while this app_process is alive. Retry a
            // mismatch against disk once, rather than serving a stale cache entry.
            synchronized(tokenLock) { cachedToken = null }
            expected = expectedToken(forceReload = true)
            valid = MessageDigest.isEqual(
                candidate.toByteArray(Charsets.US_ASCII),
                expected.toByteArray(Charsets.US_ASCII)
            )
        }
        val output = socket.getOutputStream()
        output.write((if (valid) "OK AUTH\n" else "ERR AUTH\n").toByteArray(Charsets.US_ASCII))
        output.flush()
        if (valid) socket.soTimeout = 0
        return valid
    }
}
