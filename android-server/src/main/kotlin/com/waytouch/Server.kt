package com.waytouch

import android.content.Context
import android.graphics.Point
import android.hardware.input.InputManager
import android.os.SystemClock
import android.view.InputDevice
import android.view.InputEvent
import android.view.MotionEvent
import android.view.Surface
import android.view.WindowManager
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.Semaphore
import java.util.concurrent.atomic.AtomicBoolean

private const val PORT = 27183
private const val INJECT_ASYNC = 0
private const val INJECT_WAIT_FOR_RESULT = 1
private const val DEFAULT_DISPLAY_ID = 0

internal data class AndroidDisplaySnapshot(
    val width: Int,
    val height: Int,
    val rotation: Int
)

internal object TouchInjector {
    private lateinit var inputManager: InputManager
    private lateinit var injectMethod: java.lang.reflect.Method
    private lateinit var windowManager: WindowManager
    private var mouseX = 1f
    private var mouseY = 1f
    private var screenWidth = 2
    private var screenHeight = 2
    private var screenRotation = Surface.ROTATION_0
    private var displayInitialized = false
    private var lastDisplayRefreshUptime = 0L
    private const val DISPLAY_REFRESH_INTERVAL_MS = 500L

    // MotionEvent.obtain copies pointer data into its native event. Reuse these
    // fixed arrays to avoid allocating PointerProperties/PointerCoords arrays per frame.
    private val touchProperties = Array(WTCH_MAX_POINTERS) { MotionEvent.PointerProperties() }
    private val touchCoords = Array(WTCH_MAX_POINTERS) { MotionEvent.PointerCoords() }
    // MotionEvent.obtain() does not set a target display. Android's shell input
    // command explicitly assigns DEFAULT_DISPLAY before injecting pointer events.
    private val setDisplayIdMethod by lazy {
        InputEvent::class.java.getDeclaredMethod(
            "setDisplayId", Int::class.javaPrimitiveType
        ).apply { isAccessible = true }
    }
    private val mouseProperties = arrayOf(MotionEvent.PointerProperties().apply {
        id = 0
        toolType = MotionEvent.TOOL_TYPE_MOUSE
    })
    private val mouseCoords = arrayOf(MotionEvent.PointerCoords())
    private val actionButtonMethod by lazy {
        MotionEvent::class.java.getDeclaredMethod("setActionButton", Int::class.javaPrimitiveType)
            .apply { isAccessible = true }
    }

    fun initialize() {
        val context = ShellContext.create()
        inputManager = context.getSystemService(Context.INPUT_SERVICE) as InputManager
        injectMethod = InputManager::class.java.getMethod(
            "injectInputEvent", InputEvent::class.java, Int::class.javaPrimitiveType
        )
        @Suppress("DEPRECATION")
        windowManager = context.getSystemService(Context.WINDOW_SERVICE) as WindowManager
        refreshDisplayMetrics(force = true)
        println("WTCH InputManager initialized for shell context")
        clearStaleInjectedTouchState()
        val display = displayInfo()
        println("WTCH mouse display=" + display.width + "x" + display.height + " rotation=" + display.rotation)
    }

    @Suppress("DEPRECATION")
    @Synchronized
    fun refreshDisplayMetrics(force: Boolean = false): AndroidDisplaySnapshot {
        val now = SystemClock.uptimeMillis()
        if (displayInitialized && !force && now - lastDisplayRefreshUptime < DISPLAY_REFRESH_INTERVAL_MS) {
            return AndroidDisplaySnapshot(screenWidth, screenHeight, screenRotation)
        }
        lastDisplayRefreshUptime = now
        val display = windowManager.defaultDisplay
        val size = Point().also { display.getRealSize(it) }
        val nextWidth = size.x.coerceAtLeast(2)
        val nextHeight = size.y.coerceAtLeast(2)
        val nextRotation = display.rotation

        if (!displayInitialized) {
            screenWidth = nextWidth
            screenHeight = nextHeight
            screenRotation = nextRotation
            mouseX = (screenWidth - 1) / 2f
            mouseY = (screenHeight - 1) / 2f
            displayInitialized = true
        } else if (
            nextWidth != screenWidth ||
            nextHeight != screenHeight ||
            nextRotation != screenRotation
        ) {
            var normalizedX = (mouseX / (screenWidth - 1).coerceAtLeast(1)).coerceIn(0f, 1f)
            var normalizedY = (mouseY / (screenHeight - 1).coerceAtLeast(1)).coerceIn(0f, 1f)
            val quarterTurns = (nextRotation - screenRotation + 4) % 4
            repeat(quarterTurns) {
                val previousX = normalizedX
                normalizedX = 1f - normalizedY
                normalizedY = previousX
            }

            screenWidth = nextWidth
            screenHeight = nextHeight
            screenRotation = nextRotation
            mouseX = normalizedX * (screenWidth - 1)
            mouseY = normalizedY * (screenHeight - 1)
            println("WTCH_DISPLAY_CHANGED width=$screenWidth height=$screenHeight rotation=$screenRotation")
        }
        return AndroidDisplaySnapshot(screenWidth, screenHeight, screenRotation)
    }

    fun displayInfo(): AndroidDisplaySnapshot = refreshDisplayMetrics(force = true)

    /**
     * A killed app_process cannot send its final ACTION_CANCEL. InputDispatcher
     * keeps that virtual device's pointer set alive across process restarts.
     * Probe contiguous pointer counts and cancel any matching stale stream
     * before accepting a new controller session. DeviceId 0 is Android's
     * injected virtual device (-1 in InputDispatcher), matching legacy sessions.
     */
    @Synchronized
    private fun clearStaleInjectedTouchState() {
        val now = SystemClock.uptimeMillis()
        var cleared = false
        for (count in 1..WTCH_MAX_POINTERS) {
            for (index in 0 until count) {
                touchProperties[index].apply {
                    id = index
                    toolType = MotionEvent.TOOL_TYPE_FINGER
                }
                touchCoords[index].apply {
                    x = (screenWidth - 1) / 2f
                    y = (screenHeight - 1) / 2f
                    pressure = 0f
                    size = 0f
                    touchMajor = 0f
                    touchMinor = 0f
                    orientation = 0f
                }
            }
            val event = MotionEvent.obtain(
                now,
                now,
                MotionEvent.ACTION_CANCEL,
                count,
                touchProperties,
                touchCoords,
                0,
                0,
                1f,
                1f,
                0,
                0,
                InputDevice.SOURCE_TOUCHSCREEN,
                0
            )
            try {
                prepareMotionEvent(event, InputDevice.SOURCE_TOUCHSCREEN)
                val accepted = injectMethod.invoke(inputManager, event, INJECT_WAIT_FOR_RESULT) as Boolean
                if (accepted) {
                    println("WTCH_STARTUP_TOUCH_RECOVERY accepted=true pointerCount=" + count + " deviceId=virtual")
                    cleared = true
                }
            } catch (failure: Exception) {
                System.err.println("WTCH_STARTUP_TOUCH_RECOVERY count=" + count + " error=" + failure.message)
            } finally {
                event.recycle()
            }
        }
        if (!cleared) println("WTCH_STARTUP_TOUCH_RECOVERY no_stale_touch_stream=true")
    }

    private fun inputDeviceId(source: Int): Int {
        return InputDevice.getDeviceIds().firstOrNull { id ->
            InputDevice.getDevice(id)?.supportsSource(source) == true
        } ?: 0
    }

    private fun prepareMotionEvent(event: MotionEvent, source: Int) {
        event.setSource(source)
        setDisplayIdMethod.invoke(event, DEFAULT_DISPLAY_ID)
    }

    @Synchronized
    fun inject(
        frame: TouchFrame,
        downTime: Long,
        eventTime: Long,
        nextActive: List<TouchPointer>
    ): Boolean {
        val packedAction = when (frame.action) {
            MotionEvent.ACTION_POINTER_DOWN, MotionEvent.ACTION_POINTER_UP ->
                frame.action or (frame.actionIndex shl 8)
            else -> frame.action
        }

        for (index in 0 until frame.pointerCount) {
            val pointer = frame.pointers[index]
            touchProperties[index].apply {
                id = pointer.id
                toolType = pointer.toolType
            }
            touchCoords[index].apply {
                x = pointer.x
                y = pointer.y
                val lifted = (frame.action == MotionEvent.ACTION_UP && index == 0) ||
                    (frame.action == MotionEvent.ACTION_POINTER_UP && index == frame.actionIndex) ||
                    frame.action == MotionEvent.ACTION_CANCEL
                pressure = if (lifted) 0f else pointer.pressure
                size = pointer.size
                touchMajor = pointer.size
                touchMinor = pointer.size
                orientation = 0f
            }
        }

        val event = MotionEvent.obtain(
            downTime,
            eventTime,
            packedAction,
            frame.pointerCount,
            touchProperties,
            touchCoords,
            0, // metaState
            0, // buttonState
            1f, // xPrecision
            1f, // yPrecision
            inputDeviceId(InputDevice.SOURCE_TOUCHSCREEN),
            0, // edgeFlags
            InputDevice.SOURCE_TOUCHSCREEN,
            0 // flags
        )
        prepareMotionEvent(event, InputDevice.SOURCE_TOUCHSCREEN)

        try {
            // WAIT_FOR_RESULT confirms InputDispatcher admitted the transition
            // to the focused display. It does not wait for the game to consume it.
            // MOVE remains asynchronous to keep the 240 Hz path nonblocking.
            val injectMode = if (frame.action == MotionEvent.ACTION_MOVE) {
                INJECT_ASYNC
            } else {
                INJECT_WAIT_FOR_RESULT
            }
            val accepted = injectMethod.invoke(inputManager, event, injectMode) as Boolean
            if (accepted && frame.action != MotionEvent.ACTION_MOVE) {
                println(
                    "WTCH accepted seq=${frame.sequence} action=${frame.action} " +
                        "actionIndex=${frame.actionIndex} eventPointers=${frame.pointerCount} activeAfter=${nextActive.size}"
                )
            }
            return accepted
        } finally {
            event.recycle()
        }
    }

    @Synchronized
    fun mousePosition(): Pair<Float, Float> {
        refreshDisplayMetrics()
        return Pair(mouseX, mouseY)
    }

    @Synchronized
    fun injectMouse(
        action: Int,
        buttons: Int,
        actionButton: Int,
        dx: Float,
        dy: Float,
        horizontalScroll: Float,
        verticalScroll: Float,
        downTime: Long,
        eventTime: Long
    ): Boolean {
        refreshDisplayMetrics()
        require(dx.isFinite() && dy.isFinite() && horizontalScroll.isFinite() && verticalScroll.isFinite()) {
            "mouse axes must be finite"
        }
        require(buttons in 0..31) { "invalid mouse button state" }

        if (action == MotionEvent.ACTION_HOVER_MOVE || action == MotionEvent.ACTION_MOVE) {
            mouseX = (mouseX + dx).coerceIn(0f, (screenWidth - 1).toFloat())
            mouseY = (mouseY + dy).coerceIn(0f, (screenHeight - 1).toFloat())
        }

        mouseCoords[0].apply {
            x = mouseX
            y = mouseY
            pressure = if (buttons == 0) 0f else 1f
            size = 1f
            setAxisValue(MotionEvent.AXIS_HSCROLL, horizontalScroll)
            setAxisValue(MotionEvent.AXIS_VSCROLL, verticalScroll)
        }

        val event = MotionEvent.obtain(
            downTime, eventTime, action, 1, mouseProperties, mouseCoords,
            0, buttons, 1f, 1f, inputDeviceId(InputDevice.SOURCE_MOUSE), 0,
            InputDevice.SOURCE_MOUSE, 0
        )
        prepareMotionEvent(event, InputDevice.SOURCE_MOUSE)
        try {
            if (action == MotionEvent.ACTION_BUTTON_PRESS || action == MotionEvent.ACTION_BUTTON_RELEASE) {
                actionButtonMethod.invoke(event, actionButton)
            }
            // WAIT_FOR_RESULT validates routing/admission, not app-level consumption.
            // Keep high-frequency hover/motion asynchronous; serialize button edges.
            val injectMode = when (action) {
                MotionEvent.ACTION_DOWN,
                MotionEvent.ACTION_UP,
                MotionEvent.ACTION_BUTTON_PRESS,
                MotionEvent.ACTION_BUTTON_RELEASE,
                MotionEvent.ACTION_CANCEL -> INJECT_WAIT_FOR_RESULT
                else -> INJECT_ASYNC
            }
            return injectMethod.invoke(inputManager, event, injectMode) as Boolean
        } finally {
            event.recycle()
        }
    }

    @Synchronized
    fun cancel(state: PointerState) {
        if (!state.isActive) return
        val pointers = state.pointers
        val now = SystemClock.uptimeMillis()
        val eventTime = state.nextEventTime(now)
        for (index in pointers.indices) {
            val pointer = pointers[index]
            touchProperties[index].apply {
                id = pointer.id
                toolType = pointer.toolType
            }
            touchCoords[index].apply {
                x = pointer.x
                y = pointer.y
                pressure = 0f
                size = pointer.size
                touchMajor = pointer.size
                touchMinor = pointer.size
                orientation = 0f
            }
        }
        val event = MotionEvent.obtain(
            state.downTime, eventTime, MotionEvent.ACTION_CANCEL,
            pointers.size, touchProperties, touchCoords, 0, 0, 1f, 1f,
            inputDeviceId(InputDevice.SOURCE_TOUCHSCREEN), 0,
            InputDevice.SOURCE_TOUCHSCREEN, 0
        )
        prepareMotionEvent(event, InputDevice.SOURCE_TOUCHSCREEN)
        try {
            val accepted = injectMethod.invoke(inputManager, event, INJECT_ASYNC) as Boolean
            println("WTCH disconnect-cancel accepted=$accepted pointerCount=${pointers.size}")
        } catch (e: Exception) {
            System.err.println("WTCH failed to inject disconnect CANCEL: $e")
        } finally {
            event.recycle()
            state.resetAfterCancel()
        }
    }
}

object WayTouchServer {
    private val clientActive = AtomicBoolean(false)
    private val authSlots = Semaphore(4)

    @JvmStatic
    fun main(args: Array<String>) {
        println("WayTouch Android Server v2 starting; port=$PORT")
        TouchInjector.initialize()
        LayoutConfigServer.start()
        MouseInputServer.start()
        ServerSocket(PORT, 4, InetAddress.getByName("127.0.0.1")).use { server ->
            println("WTCH_READY version=2 maxPointers=$WTCH_MAX_POINTERS")
            while (true) {
                val socket = server.accept()
                socket.tcpNoDelay = true
                socket.keepAlive = true
                socket.soTimeout = 3000
                if (!authSlots.tryAcquire()) {
                    println("WTCH rejected client: authentication slots are full")
                    socket.close()
                    continue
                }
                Thread({
                    var reserved = false
                    try {
                        if (!IpcAuth.authenticate(socket)) {
                            println("WTCH rejected unauthenticated client")
                            return@Thread
                        }
                        if (!clientActive.compareAndSet(false, true)) {
                            println("WTCH rejected extra client: controller already connected")
                            return@Thread
                        }
                        reserved = true
                        println("WTCH client connected: ${socket.inetAddress}")
                        handleAuthenticated(socket)
                    } catch (failure: Exception) {
                        System.err.println("WTCH pre-auth/client error type=" +
                            failure.javaClass.name + ": " + failure.message)
                    } finally {
                        if (reserved) clientActive.set(false)
                        authSlots.release()
                        try { socket.close() } catch (_: Exception) {}
                    }
                }, "waytouch-controller-auth").start()
            }
        }
    }

    private fun writeAck(output: DataOutputStream, sequence: Long) {
        output.write(byteArrayOf(0x57, 0x54, 0x4f, 0x4b))
        output.writeInt(Integer.reverseBytes(sequence.toInt()))
        output.flush()
    }

    private fun handleAuthenticated(socket: Socket) {
        val state = PointerState()
        socket.use { client ->
            val input = DataInputStream(BufferedInputStream(client.getInputStream(), 64 * 1024))
            val output = DataOutputStream(BufferedOutputStream(client.getOutputStream(), 8 * 1024))
            var acceptedMoves = 0L
            var droppedMoves = 0L
            var moveStatsStarted = SystemClock.uptimeMillis()
            fun reportMoveStatsIfDue() {
                val now = SystemClock.uptimeMillis()
                if (now - moveStatsStarted >= 5000L) {
                    println(
                        "WTCH_TOUCH_STATS accepted_moves=" + acceptedMoves +
                            " dropped_moves=" + droppedMoves +
                            " window_ms=" + (now - moveStatsStarted)
                    )
                    acceptedMoves = 0L
                    droppedMoves = 0L
                    moveStatsStarted = now
                }
            }
            try {
                // Authentication completed before the single-controller slot was reserved.
                client.soTimeout = 0
                client.keepAlive = true
                while (true) {
                    val magic = ByteArray(4)
                    val first = input.read()
                    if (first < 0) break
                    magic[0] = first.toByte()
                    input.readFully(magic, 1, 3)

                    val frame = TouchFrame.read(input, magic)
                    val nextActive = state.validate(frame)
                    val now = SystemClock.uptimeMillis()
                    val downTime = if (frame.action == MotionEvent.ACTION_DOWN) now else state.downTime
                    val eventTime = state.nextEventTime(now)

                    val accepted = TouchInjector.inject(frame, downTime, eventTime, nextActive)
                    if (!accepted && frame.action == MotionEvent.ACTION_MOVE) {
                        // MOVE carries only the latest pointer snapshot; losing one
                        // must not tear down the controller. Commit its logical state
                        // and ACK so the next accepted MOVE naturally catches up.
                        state.commit(frame, nextActive, downTime, eventTime)
                        droppedMoves += 1L
                        writeAck(output, frame.sequence)
                        reportMoveStatsIfDue()
                        continue
                    }
                    check(accepted) {
                        "InputManager rejected critical sequence=${frame.sequence}, action=${frame.action}"
                    }
                    state.commit(frame, nextActive, downTime, eventTime)
                    if (frame.action == MotionEvent.ACTION_MOVE) acceptedMoves += 1L
                    writeAck(output, frame.sequence)
                    reportMoveStatsIfDue()
                }
                println("WTCH client disconnected cleanly")
            } catch (e: Exception) {
                System.err.println("WTCH protocol/client error type=" + e.javaClass.name + ": " + e.message)
                e.printStackTrace()
            } finally {
                TouchInjector.cancel(state)
            }
        }
    }
}
