package com.waytouch

import android.os.SystemClock
import android.view.MotionEvent
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

/**
 * Dedicated Android SOURCE_MOUSE injector. The wire format is line-oriented
 * and separate from the WTCH multitouch protocol:
 * WTM3 sequence action buttonState actionButton dx dy hScroll vScroll
 */
internal object MouseInputServer {
    private const val PORT = 27186
    private const val MAX_LINE_LENGTH = 256
    private val FIELD_SEPARATOR = Regex("\\s+")
    private val SUPPORTED_ACTIONS = setOf(3, 7, 8, 9, 10, 11, 12, 13)
    private val DROPPABLE_ACTIONS = setOf(3, 7, 8, 9, 10)
    private val controllerActive = AtomicBoolean(false)
    // One worker owns the persistent input stream; the second remains free
    // for short authenticated PING probes while that stream is idle or active.
    private val clients = ThreadPoolExecutor(
        2, 2, 30L, TimeUnit.SECONDS, ArrayBlockingQueue(4),
        ThreadPoolExecutor.AbortPolicy()
    )

    private class ClientState {
        var buttons = 0
        var downTime = SystemClock.uptimeMillis()
        var lastEventTime = downTime
        var lastSequence = 0L

        fun nextEventTime(): Long {
            val now = SystemClock.uptimeMillis()
            lastEventTime = maxOf(now, lastEventTime)
            return lastEventTime
        }
    }

    private fun writeAck(output: java.io.OutputStream, sequence: Long) {
        val position = TouchInjector.mousePosition()
        output.write(
            ("WTMA3 " + sequence + " " + position.first + " " + position.second + "\n")
                .toByteArray(Charsets.US_ASCII)
        )
        output.flush()
    }

    fun start() {
        thread(name = "waytouch-android-mouse-server", isDaemon = true) {
            try {
                ServerSocket(PORT, 8, InetAddress.getByName("127.0.0.1")).use { server ->
                    println("WTCH_MOUSE_READY port=" + PORT)
                    while (true) {
                        val client = server.accept()
                        try {
                            clients.execute { handle(client) }
                        } catch (_: RejectedExecutionException) {
                            System.err.println("WTCH mouse rejected excess client")
                            client.close()
                        }
                    }
                }
            } catch (error: Exception) {
                System.err.println("WTCH mouse server failed: " + error)
            }
        }
    }

    private fun handle(socket: Socket) {
        val state = ClientState()
        var reportedActive = false
        var ownsController = false
        socket.use { client ->
            try {
                if (!IpcAuth.authenticate(client)) return
                client.keepAlive = true
                val input = BufferedInputStream(client.getInputStream(), 8 * 1024)
                val output = client.getOutputStream()

                // A health probe must be serviceable while the real controller
                // keeps its persistent WTM3 stream open. It never owns the input
                // slot and never injects or queries a MotionEvent.
                client.soTimeout = 1_000
                var line = readBoundedLine(input, MAX_LINE_LENGTH) ?: return
                if (line == "PING") {
                    output.write("PONG\n".toByteArray(Charsets.US_ASCII))
                    output.flush()
                    return
                }
                // Older GUI versions use a one-shot WTM3 position query for
                // their health probe. It remains read-only and does not own the
                // persistent controller slot.
                if (line.startsWith("WTM3 ")) {
                    val probeFields = line.trim().split(FIELD_SEPARATOR)
                    if (probeFields.size == 9 && probeFields[0] == "WTM3" && probeFields[2] == "13") {
                        val sequence = probeFields[1].toLongOrNull()
                        val zeroPayload = probeFields.drop(3).all { it.toFloatOrNull() == 0f }
                        if (sequence != null && sequence in 1..0xffff_ffffL && zeroPayload) {
                            writeAck(output, sequence)
                            return
                        }
                    }
                    System.err.println("WTCH mouse rejected controller without CONTROL registration")
                    return
                }
                if (line != "CONTROL") {
                    System.err.println("WTCH mouse rejected unknown initial command")
                    return
                }
                if (!controllerActive.compareAndSet(false, true)) {
                    output.write("ERR BUSY\n".toByteArray(Charsets.US_ASCII))
                    output.flush()
                    return
                }
                ownsController = true
                client.soTimeout = 0
                output.write("OK CONTROL\n".toByteArray(Charsets.US_ASCII))
                output.flush()
                line = readBoundedLine(input, MAX_LINE_LENGTH) ?: return

                var acceptedMotion = 0L
                var droppedMotion = 0L
                var statsStarted = SystemClock.uptimeMillis()
                fun reportStatsIfDue() {
                    val now = SystemClock.uptimeMillis()
                    if (now - statsStarted >= 5000L) {
                        println(
                            "WTCH_MOUSE_STATS accepted_motion=" + acceptedMotion +
                                " dropped_motion=" + droppedMotion +
                                " window_ms=" + (now - statsStarted)
                        )
                        acceptedMotion = 0L
                        droppedMotion = 0L
                        statsStarted = now
                    }
                }
                while (true) {
                    val fields = line.trim().split(FIELD_SEPARATOR)
                    require(fields.size == 9 && fields[0] == "WTM3") { "invalid mouse frame format" }
                    val sequence = fields[1].toLong()
                    val nextSequence = if (state.lastSequence >= 0xffff_ffffL) {
                        1L
                    } else {
                        state.lastSequence + 1L
                    }
                    require(sequence > 0L && sequence == nextSequence) {
                        "mouse sequence mismatch: expected " + nextSequence + " received " + sequence
                    }

                    val rawAction = fields[2].toInt()
                    val nextButtons = fields[3].toInt()
                    val actionButton = fields[4].toInt()
                    val dx = fields[5].toFloat()
                    val dy = fields[6].toFloat()
                    val hScroll = fields[7].toFloat()
                    val vScroll = fields[8].toFloat()
                    require(rawAction in SUPPORTED_ACTIONS) { "unsupported mouse action " + rawAction }
                    require(nextButtons in 0..31) { "invalid mouse button state" }
                    require(actionButton in 0..31) { "invalid mouse action button" }
                    require(dx.isFinite() && dy.isFinite() && hScroll.isFinite() && vScroll.isFinite()) {
                        "mouse axes must be finite"
                    }
                    require(dx in -8192f..8192f && dy in -8192f..8192f) { "mouse delta too large" }
                    require(hScroll in -1000f..1000f && vScroll in -1000f..1000f) { "mouse wheel delta too large" }

                    if (rawAction == 13) {
                        // Position query is a protocol barrier: it changes no input state.
                        state.lastSequence = sequence
                        writeAck(output, sequence)
                        line = readBoundedLine(input, MAX_LINE_LENGTH) ?: break
                        continue
                    }

                    val eventTime = state.nextEventTime()
                    // Android's InputDispatcher synthesizes hover enter/exit from
                    // SOURCE_MOUSE HOVER_MOVE. Injecting HOVER_ENTER/EXIT directly
                    // is rejected by some Android builds, so raw 10 is an explicit
                    // host-side hide transition and raw 9 is a zero-delta hover move.
                    if (rawAction == 10) {
                        println("WTCH_MOUSE_POINTER_MODE hidden_by_host=true")
                        state.lastSequence = sequence
                        writeAck(output, sequence)
                        line = readBoundedLine(input, MAX_LINE_LENGTH) ?: break
                        continue
                    }
                    val action = when (rawAction) {
                        3 -> MotionEvent.ACTION_CANCEL
                        7, 9 -> if (nextButtons == 0) MotionEvent.ACTION_HOVER_MOVE else MotionEvent.ACTION_MOVE
                        8 -> MotionEvent.ACTION_SCROLL
                        // Android starts a pointer gesture with ACTION_DOWN for
                        // the first pressed mouse button, regardless of whether
                        // it is primary, secondary or tertiary. buttonState tells
                        // the app which button began the gesture. BUTTON_PRESS is
                        // only used when another mouse button is already held.
                        11 -> if (state.buttons == 0) {
                            MotionEvent.ACTION_DOWN
                        } else MotionEvent.ACTION_BUTTON_PRESS
                        // End the gesture with ACTION_UP when the final button is
                        // released; intermediate releases remain BUTTON_RELEASE.
                        12 -> if (nextButtons == 0) {
                            MotionEvent.ACTION_UP
                        } else MotionEvent.ACTION_BUTTON_RELEASE
                        else -> error("unreachable mouse action")
                    }

                    val eventButtons = if (rawAction == 9 || rawAction == 10) state.buttons else nextButtons
                    // One Android mouse gesture spans the first pressed button
                    // through the final release. Secondary-first clicks and CANCEL
                    // must preserve the same downTime as the active gesture.
                    if ((state.buttons == 0 && nextButtons != 0) || action == MotionEvent.ACTION_DOWN) {
                        state.downTime = eventTime
                    }
                    val eventDownTime = if (
                        action == MotionEvent.ACTION_UP ||
                        action == MotionEvent.ACTION_CANCEL ||
                        action == MotionEvent.ACTION_BUTTON_PRESS ||
                        action == MotionEvent.ACTION_BUTTON_RELEASE ||
                        action == MotionEvent.ACTION_MOVE
                    ) state.downTime else eventTime

                    val accepted = TouchInjector.injectMouse(
                        action = action,
                        buttons = eventButtons,
                        actionButton = actionButton,
                        dx = dx,
                        dy = dy,
                        horizontalScroll = hScroll,
                        verticalScroll = vScroll,
                        downTime = eventDownTime,
                        eventTime = eventTime
                    )
                    if (!accepted && rawAction in DROPPABLE_ACTIONS) {
                        // Hover/scroll/cancel are state snapshots or cleanup. An
                        // undeliverable frame is safe to drop; keep the WTM3 stream
                        // alive so the next accepted frame restores the pointer.
                        if (rawAction == 3) state.buttons = 0
                        state.lastSequence = sequence
                        if (rawAction == 7 || rawAction == 8 || rawAction == 9) droppedMotion += 1L
                        writeAck(output, sequence)
                        reportStatsIfDue()
                        line = readBoundedLine(input, MAX_LINE_LENGTH) ?: break
                        continue
                    }
                    check(accepted) {
                        "InputManager rejected mouse transition sequence=$sequence action=$rawAction"
                    }
                    state.buttons = if (rawAction == 3) 0 else eventButtons
                    state.lastSequence = sequence
                    acceptedMotion += if (rawAction == 7 || rawAction == 8 || rawAction == 9) 1L else 0L
                    writeAck(output, sequence)
                    reportStatsIfDue()
                    if (!reportedActive) {
                        reportedActive = true
                        println(
                            "WTCH_MOUSE_ACTIVE action=" + rawAction +
                                " buttons=" + eventButtons +
                                " pointer=" + if (rawAction == 9 || rawAction == 7) "hover" else "button"
                        )
                    }
                    if (rawAction == 11 || rawAction == 12) {
                        println(
                            "WTCH_MOUSE_BUTTON_ACCEPTED action=" + rawAction +
                                " actionButton=" + actionButton + " buttons=" + eventButtons
                        )
                    }
                    line = readBoundedLine(input, MAX_LINE_LENGTH) ?: break
                }
                println("WTCH mouse client disconnected cleanly")
            } catch (error: Exception) {
                System.err.println("WTCH mouse client error type=" + error.javaClass.name + ": " + error.message)
            } finally {
                if (ownsController) controllerActive.set(false)
                if (state.buttons != 0) {
                    try {
                        val now = state.nextEventTime()
                        TouchInjector.injectMouse(
                            action = MotionEvent.ACTION_CANCEL,
                            buttons = 0,
                            actionButton = 0,
                            dx = 0f,
                            dy = 0f,
                            horizontalScroll = 0f,
                            verticalScroll = 0f,
                            downTime = state.downTime,
                            eventTime = now
                        )
                    } catch (error: Exception) {
                        System.err.println("WTCH mouse disconnect cancel failed: " + error)
                    }
                }
            }
        }
    }
}
