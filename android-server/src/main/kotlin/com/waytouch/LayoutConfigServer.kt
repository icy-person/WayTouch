package com.waytouch

import android.system.Os
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.File
import java.io.FileOutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/**
 * Revisioned, loopback-only layout channel shared by Android editor, Linux GUI
 * and Rust mapper.
 *
 * GET                          -> {"revision":N,"layout":{...}}
 * SET <expected> <JSON>        -> compare-and-swap Android edit, increments revision
 * SYNC <expected> <new> <JSON> -> compare-and-swap Linux layout publication
 */
internal object LayoutConfigServer {
    private const val PORT = 27184
    private const val MAX_LINE_BYTES = 16 * 1024
    private val stateFile = File("/data/local/tmp/waytouch-layout.json")
    private val stateLock = Any()
    private val clients = ThreadPoolExecutor(
        4, 4, 30L, TimeUnit.SECONDS, ArrayBlockingQueue(8),
        ThreadPoolExecutor.AbortPolicy()
    )

    private data class LayoutState(val revision: Long, val layout: String)

    @Volatile
    private var currentState: LayoutState? = null

    fun start() {
        currentState = loadFromDisk()
        thread(name = "waytouch-layout-config", isDaemon = true) {
            try {
                // Use IPv4 loopback: Android may otherwise choose ::1 while the
                // editor and ADB forward connect to 127.0.0.1.
                ServerSocket(PORT, 8, InetAddress.getByName("127.0.0.1")).use { server ->
                    println("WTCH_LAYOUT_READY port=$PORT revision=" + (currentState?.revision ?: -1L))
                    while (true) {
                        val client = server.accept()
                        try {
                            clients.execute { handle(client) }
                        } catch (_: RejectedExecutionException) {
                            System.err.println("WTCH layout rejected excess client")
                            client.close()
                        }
                    }
                }
            } catch (error: Exception) {
                System.err.println("WTCH layout config server failed: $error")
            }
        }
    }

    private fun handle(socket: Socket) {
        socket.use { client ->
            client.tcpNoDelay = true
            client.soTimeout = 750
            if (!IpcAuth.authenticate(client)) return
            // Persistent authenticated host connection. A bounded idle timeout
            // lets dead peers release their worker; the host reconnects on EOF.
            client.soTimeout = 5000
            val input = BufferedInputStream(client.getInputStream(), 16 * 1024)
            val output = client.getOutputStream()
            while (true) {
                val line = try {
                    readBoundedLine(input, MAX_LINE_BYTES)
                } catch (_: java.net.SocketTimeoutException) {
                    break
                } ?: break
                if (line == "PING") {
                    writeReply(output, "PONG")
                    continue
                }
                val reply = try {
                    when {
                        line == "GET" -> {
                            val state = currentState
                            if (state == null) "NONE"
                            else JSONObject()
                                .put("revision", state.revision)
                                .put("layout", JSONObject(state.layout))
                                .toString()
                        }
                        line == "DISPLAY" -> {
                            val display = TouchInjector.displayInfo()
                            JSONObject()
                                .put("width", display.width)
                                .put("height", display.height)
                                .put("rotation", display.rotation)
                                .toString()
                        }
                        line.startsWith("SET ") -> applyAndroidEdit(line)
                        line.startsWith("SYNC ") -> applyLinuxSync(line)
                        line.startsWith("SEED ") -> seedInitialLayout(line)
                        else -> "ERR unknown command"
                    }
                } catch (error: Exception) {
                    "ERR " + (error.message ?: "invalid request").replace('\n', ' ')
                }
                writeReply(output, reply)
            }
        }
    }

    private fun applyAndroidEdit(line: String): String = synchronized(stateLock) {
        val first = line.indexOf(' ', 4)
        require(first > 4 && first < line.lastIndex) { "SET requires expected revision and JSON" }
        val expected = line.substring(4, first).toLong()
        require(expected >= 0L) { "revision must be non-negative" }
        val normalized = normalize(line.substring(first + 1))
        val current = currentState ?: return@synchronized "CONFLICT -1"
        if (expected != current.revision) return@synchronized "CONFLICT " + current.revision
        val nextRevision = current.revision + 1L
        val next = LayoutState(nextRevision, normalized)
        persist(next)
        currentState = next
        println("WTCH_LAYOUT_UPDATED source=android revision=$nextRevision")
        "OK $nextRevision"
    }

    private fun seedInitialLayout(line: String): String = synchronized(stateLock) {
        require(line.length > 5) { "SEED requires a JSON payload" }
        if (currentState != null) return@synchronized "CONFLICT " + currentState!!.revision
        val normalized = normalize(line.substring(5))
        val initial = LayoutState(0L, normalized)
        persist(initial)
        currentState = initial
        println("WTCH_LAYOUT_RECOVERED source=linux-default revision=0")
        "OK 0"
    }

    private fun applyLinuxSync(line: String): String = synchronized(stateLock) {
        val first = line.indexOf(' ', 5)
        require(first > 5) { "SYNC requires expected and new revisions" }
        val second = line.indexOf(' ', first + 1)
        require(second > first + 1 && second < line.lastIndex) { "SYNC requires JSON payload" }
        val expected = line.substring(5, first).toLong()
        val nextRevision = line.substring(first + 1, second).toLong()
        require(expected >= 0L && nextRevision > expected) { "SYNC revisions are invalid" }
        val normalized = normalize(line.substring(second + 1))
        val current = currentState ?: return@synchronized "CONFLICT -1"
        if (expected != current.revision) return@synchronized "CONFLICT " + current.revision
        if (nextRevision <= current.revision) return@synchronized "CONFLICT " + current.revision
        val next = LayoutState(nextRevision, normalized)
        persist(next)
        currentState = next
        println("WTCH_LAYOUT_UPDATED source=linux revision=$nextRevision")
        "OK $nextRevision"
    }

    private fun writeReply(output: java.io.OutputStream, value: String) {
        output.write((value + "\n").toByteArray(Charsets.UTF_8))
        output.flush()
    }

    private fun normalize(raw: String): String {
        require(raw.toByteArray(Charsets.UTF_8).size <= MAX_LINE_BYTES) { "layout payload too large" }
        val root = JSONObject(raw)
        require(root.length() == 2 && root.has("movement") && root.has("buttons")) {
            "layout requires movement and buttons only"
        }

        val movement = root.getJSONObject("movement")
        require(
            movement.has("center") && movement.has("radius")
                && ((movement.has("keys") && movement.length() == 3)
                    || (!movement.has("keys") && movement.length() == 2))
        ) { "movement requires center and radius, with optional keys" }
        val center = movement.getJSONArray("center")
        require(center.length() == 2) { "movement.center must have two values" }
        val x = center.getDouble(0)
        val y = center.getDouble(1)
        val radius = movement.getDouble("radius")
        require(x.isFinite() && y.isFinite() && x in 0.0..1.0 && y in 0.0..1.0) {
            "movement center must be normalized to [0, 1]"
        }
        require(radius.isFinite() && radius in 0.001..0.5) {
            "movement radius must be in [0.001, 0.5]"
        }

        val cleanMovement = JSONObject()
            .put("center", JSONArray().put(x).put(y))
            .put("radius", radius)
        val movementCodes = HashSet<Int>()
        // Linux evdev keycodes: Ctrl/Alt/Esc emergency chord and F2 global mouse lock.
        val reservedKeys = setOf(1, 29, 56, 60, 97, 100)
        if (movement.has("keys")) {
            val keys = movement.getJSONObject("keys")
            require(keys.length() == 4 && listOf("up", "left", "down", "right").all(keys::has)) {
                "movement.keys requires up, left, down and right"
            }
            val cleanKeys = JSONObject()
            for (name in listOf("up", "left", "down", "right")) {
                val code = keys.getInt(name)
                require(code in 0..767 && !reservedKeys.contains(code) && movementCodes.add(code)) {
                    "movement keys must be unique and cannot use Ctrl, Alt, Escape or reserved F2"
                }
                cleanKeys.put(name, code)
            }
            cleanMovement.put("keys", cleanKeys)
        }

        val buttons = root.getJSONArray("buttons")
        require(buttons.length() <= 9) { "at most nine action buttons are supported" }
        val seen = HashSet<String>()
        val triggers = HashSet<String>()
        val cleanButtons = JSONArray()
        for (index in 0 until buttons.length()) {
            val item = buttons.getJSONObject(index)
            val hasSource = item.has("source")
            val hasCode = item.has("code")
            require(
                item.has("name") && item.has("position")
                    && (item.length() == 2 || (item.length() == 4 && hasSource && hasCode))
                    && hasSource == hasCode
            ) { "button entries require name, position and optional source/code pair" }
            val name = item.getString("name").trim()
            require(name.isNotEmpty() && name.codePointCount(0, name.length) <= 40 && seen.add(name)) {
                "button names must be non-empty, unique and at most 40 characters"
            }
            val position = item.getJSONArray("position")
            require(position.length() == 2) { "button position must have two values" }
            val bx = position.getDouble(0)
            val by = position.getDouble(1)
            require(bx.isFinite() && by.isFinite() && bx in 0.0..1.0 && by in 0.0..1.0) {
                "button '$name' position must be normalized to [0, 1]"
            }
            val cleanButton = JSONObject()
                .put("name", name)
                .put("position", JSONArray().put(bx).put(by))
            if (hasSource) {
                val source = item.getString("source")
                val code = item.getInt("code")
                val validCode = when (source) {
                    "key" -> code in 0..767 && !reservedKeys.contains(code)
                        && (movementCodes.isEmpty() || !movementCodes.contains(code))
                    "mouse" -> code in 272..287
                    else -> false
                }
                require(validCode) { "button '$name' has an invalid source/code pair" }
                require(triggers.add("$source:$code")) { "duplicate input trigger $source:$code" }
                cleanButton.put("source", source).put("code", code)
            }
            cleanButtons.put(cleanButton)
        }

        return JSONObject().put("movement", cleanMovement).put("buttons", cleanButtons).toString()
    }

    private fun loadFromDisk(): LayoutState? {
        if (!stateFile.isFile) return null
        return try {
            val root = JSONObject(stateFile.readText(Charsets.UTF_8))
            if (root.has("revision") && root.has("layout")) {
                val revision = root.getLong("revision")
                require(revision >= 0L) { "saved layout revision is invalid" }
                LayoutState(revision, normalize(root.getJSONObject("layout").toString()))
            } else {
                // Migrate the original unversioned file once, preserving its layout.
                LayoutState(0L, normalize(root.toString())).also {
                    persist(it)
                    println("WTCH_LAYOUT_MIGRATED revision=0")
                }
            }
        } catch (error: Exception) {
            val backup = File(
                stateFile.parentFile,
                stateFile.name + ".corrupt." + System.currentTimeMillis()
            )
            val moved = try { stateFile.renameTo(backup) } catch (_: Exception) { false }
            System.err.println(
                "WTCH saved Android layout is corrupt: $error; " +
                    if (moved) "preserved_as=" + backup.absolutePath else "backup_failed"
            )
            null
        }
    }

    private fun persist(state: LayoutState) {
        val parent = stateFile.parentFile ?: error("invalid layout state path")
        check(parent.exists() || parent.mkdirs()) { "cannot create layout state directory" }
        val envelope = JSONObject()
            .put("revision", state.revision)
            .put("layout", JSONObject(state.layout))
            .toString()
        val temp = File(parent, stateFile.name + ".tmp")
        try {
            FileOutputStream(temp).use { stream ->
                stream.write(envelope.toByteArray(Charsets.UTF_8))
                stream.fd.sync()
            }
            // Same-directory rename atomically replaces the previous envelope.
            Os.rename(temp.absolutePath, stateFile.absolutePath)
        } finally {
            if (temp.exists()) temp.delete()
        }
    }
}
