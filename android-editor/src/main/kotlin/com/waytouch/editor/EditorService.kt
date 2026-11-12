package com.waytouch.editor

import android.app.Notification
import android.app.NotificationChannel
import android.app.AlertDialog
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.ComponentCallbacks2
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.PixelFormat
import android.graphics.DashPathEffect
import android.graphics.RadialGradient
import android.graphics.Point
import android.graphics.RectF
import android.graphics.Typeface
import android.os.Build
import android.os.Handler
import android.os.SystemClock
import android.os.Looper
import android.os.IBinder
import android.provider.Settings
import android.util.Log
import android.view.Gravity
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.view.WindowManager
import android.widget.Toast
import android.widget.ArrayAdapter
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.Spinner
import android.widget.TextView
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedInputStream
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.Semaphore
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import java.util.concurrent.TimeUnit
import java.security.MessageDigest
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.round
import kotlin.math.sqrt

private val CURSOR_FIELD_SEPARATOR = Regex("\\s+")
private val CURSOR_STYLES = setOf("crosshair", "pointer", "dot")

private object WayTouchDesign {
    const val PANEL = 0xFF131B29.toInt()
    const val PANEL_ALT = 0xFF16202F.toInt()
    const val STROKE = 0xFF24344A.toInt()
    const val TEXT = 0xFFE8F0F8.toInt()
    const val TEXT_MUTED = 0xFF93A8BD.toInt()
    const val ACCENT = 0xFF3DD6C3.toInt()
    const val ACCENT_DARK = 0xFF1FA48D.toInt()
    const val DANGER = 0xFFE25C5C.toInt()
    const val AMBER = 0xFFE8B34B.toInt()
}


private fun readEditorBoundedLine(input: InputStream, maxBytes: Int): String? {
    val bytes = ByteArrayOutputStream(minOf(maxBytes, 256))
    var consumed = 0
    while (true) {
        val value = input.read()
        if (value < 0) {
            if (consumed == 0) return null
            throw java.io.EOFException("connection closed before line terminator")
        }
        if (value == 10) return bytes.toString(Charsets.UTF_8.name())
        consumed += 1
        if (consumed > maxBytes) throw IllegalArgumentException("cursor request too large")
        if (value != 13) bytes.write(value)
    }
}

private fun authenticateCursorClient(socket: Socket, token: String): Boolean {
    val input = socket.getInputStream()
    val supplied = readEditorBoundedLine(input, 80) ?: return false
    val candidate = if (supplied.startsWith("AUTH ")) supplied.removePrefix("AUTH ") else ""
    val valid = supplied.startsWith("AUTH ") && MessageDigest.isEqual(
        candidate.toByteArray(Charsets.US_ASCII),
        token.toByteArray(Charsets.US_ASCII)
    )
    val output = socket.getOutputStream()
        output.write((if (valid) "OK AUTH\n" else "ERR AUTH\n").toByteArray(Charsets.US_ASCII))
    output.flush()
    return valid
}

private data class EditorButton(
    var name: String,
    var x: Float,
    var y: Float,
    var source: String,
    var code: Int
)

private data class TriggerOption(val label: String, val source: String, val code: Int)
private data class LoadedLayout(val revision: Long, val layout: EditorLayout)

private data class EditorLayout(
    var centerX: Float = 0.16f,
    var centerY: Float = 0.78f,
    var radius: Float = 0.07f,
    var upKey: Int = 17,
    var leftKey: Int = 30,
    var downKey: Int = 31,
    var rightKey: Int = 32,
    val buttons: MutableList<EditorButton> = mutableListOf(
        EditorButton("jump", 0.83f, 0.81f, "key", 57),
        EditorButton("reload", 0.91f, 0.68f, "mouse", 272)
    )
) {
    fun toJson(): String {
        val buttonArray = JSONArray()
        buttons.forEach { item ->
            buttonArray.put(
                JSONObject()
                    .put("name", item.name)
                    .put("position", JSONArray().put(item.x.toDouble()).put(item.y.toDouble()))
                    .put("source", item.source)
                    .put("code", item.code)
            )
        }
        val keys = JSONObject()
            .put("up", upKey).put("left", leftKey).put("down", downKey).put("right", rightKey)
        val movement = JSONObject()
            .put("center", JSONArray().put(centerX.toDouble()).put(centerY.toDouble()))
            .put("radius", radius.toDouble())
            .put("keys", keys)
        return JSONObject().put("movement", movement).put("buttons", buttonArray).toString()
    }

    companion object {
        fun fromJson(raw: String): EditorLayout {
            val root = JSONObject(raw)
            val movement = root.getJSONObject("movement")
            val center = movement.getJSONArray("center")
            val keys = movement.optJSONObject("keys")
            val result = EditorLayout(
                centerX = center.getDouble(0).toFloat(),
                centerY = center.getDouble(1).toFloat(),
                radius = movement.getDouble("radius").toFloat(),
                upKey = keys?.optInt("up", 17) ?: 17,
                leftKey = keys?.optInt("left", 30) ?: 30,
                downKey = keys?.optInt("down", 31) ?: 31,
                rightKey = keys?.optInt("right", 32) ?: 32,
                buttons = mutableListOf()
            )
            val array = root.getJSONArray("buttons")
            for (i in 0 until array.length()) {
                val item = array.getJSONObject(i)
                val position = item.getJSONArray("position")
                val name = item.getString("name")
                val defaultSource = if (name.equals("reload", ignoreCase = true)) "mouse" else "key"
                val defaultCode = if (defaultSource == "mouse") 272 else 57
                result.buttons += EditorButton(
                    name,
                    position.getDouble(0).toFloat(),
                    position.getDouble(1).toFloat(),
                    item.optString("source", defaultSource),
                    item.optInt("code", defaultCode)
                )
            }
            return result
        }
    }
}

class EditorService : Service() {
    private var layoutRevision: Long = 0L
    private var windowManager: WindowManager? = null
    private var editorView: LayoutEditorView? = null
    private var overlayAttached = false
    @Volatile private var saveInProgress = false
    @Volatile private var loadingLayout = false
    @Volatile private var editorRequestGeneration = 0L
    @Volatile private var testingGame = false
    private var testRestoreRunnable: Runnable? = null
    private var notificationManager: NotificationManager? = null
    private var cursorView: CursorOverlayView? = null
    private var cursorWindowParams: WindowManager.LayoutParams? = null
    @Volatile private var cursorServerRunning = false
    @Volatile private var cursorServerStarting = false
    @Volatile private var cursorServerSocket: java.net.ServerSocket? = null
    @Volatile private var cursorClientSocket: java.net.Socket? = null
    @Volatile private var cursorExecutor: ThreadPoolExecutor? = null
    private val cursorClientActive = AtomicBoolean(false)
    private val cursorConnectionSlots = Semaphore(4)
    private val cursorClients = ConcurrentHashMap.newKeySet<Socket>()
    @Volatile private var serviceDestroyed = false
    @Volatile private var ipcToken: String? = null

    override fun onCreate() {
        super.onCreate()
        ipcToken = getSharedPreferences(IPC_PREFS, MODE_PRIVATE).getString(IPC_TOKEN_KEY, null)
        windowManager = getSystemService(WINDOW_SERVICE) as WindowManager
        notificationManager = getSystemService(NotificationManager::class.java)
        startCursorServer()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            notificationManager?.createNotificationChannel(NotificationChannel(
                CHANNEL_ID, "WayTouch controls", NotificationManager.IMPORTANCE_LOW
            ).apply {
                description = "Quick access to WayTouch touch-layout editing"
                setShowBadge(false)
            })
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        intent?.getStringExtra(EXTRA_IPC_TOKEN)?.let { supplied ->
            if (supplied.length == 64 && supplied.all { it in '0'..'9' || it.lowercaseChar() in 'a'..'f' }) {
                ipcToken = supplied.lowercase()
                // The in-memory token is already authoritative for this process.
                // apply() persists it asynchronously without blocking the service main thread.
                getSharedPreferences(IPC_PREFS, MODE_PRIVATE).edit()
                    .putString(IPC_TOKEN_KEY, ipcToken).apply()
                Log.i(TAG, "WayTouch IPC session token accepted")
            } else {
                Log.w(TAG, "Ignoring malformed WayTouch IPC session token")
            }
        }
        startForeground(NOTIFICATION_ID, buildNotification())
        when (intent?.action) {
            ACTION_OPEN -> showEditor()
            ACTION_CLOSE -> hideEditor()
            ACTION_STOP -> {
                hideEditor(updateNotification = false)
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
        }
        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        Log.w(TAG, "Android memory trim callback level=$level")
        if (level == ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW ||
            level == ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL
        ) {
            // The custom cursor is transient. Drop its graphics window first;
            // the next cursor SHOW recreates it without restarting the service.
            mainHandler.post {
                if (!serviceDestroyed) removeCursorImmediately()
            }
        }
    }

    override fun onLowMemory() {
        super.onLowMemory()
        Log.e(TAG, "Android low-memory callback received; releasing cursor surface")
        mainHandler.post {
            if (!serviceDestroyed) removeCursorImmediately()
        }
    }

    override fun onDestroy() {
        serviceDestroyed = true
        cursorServerRunning = false
        pendingCursorShow.set(null)
        try { cursorClientSocket?.close() } catch (_: Exception) {}
        cursorClientSocket = null
        cursorClients.forEach { client -> try { client.close() } catch (_: Exception) {} }
        cursorClients.clear()
        cursorExecutor?.shutdownNow()
        cursorExecutor = null
        hideEditor(updateNotification = false)
        try { cursorServerSocket?.close() } catch (_: Exception) {}
        cursorServerSocket = null
        removeCursorImmediately()
        super.onDestroy()
    }

    private fun startCursorServer() {
        if (cursorServerRunning || cursorServerStarting || serviceDestroyed) return
        cursorServerStarting = true
        Thread({
            var executor: ThreadPoolExecutor? = null
            try {
                java.net.ServerSocket(
                    CURSOR_PORT, 4, java.net.InetAddress.getByName("127.0.0.1")
                ).use { server ->
                    cursorServerSocket = server
                    cursorServerRunning = true
                    cursorServerStarting = false
                    val workerPool = ThreadPoolExecutor(
                        4, 4, 0L, TimeUnit.MILLISECONDS,
                        ArrayBlockingQueue(4),
                        { task -> Thread(task, "waytouch-cursor-client").apply { isDaemon = true } },
                        ThreadPoolExecutor.AbortPolicy()
                    )
                    executor = workerPool
                    cursorExecutor = workerPool
                    Log.i(TAG, "Custom cursor control channel ready on 127.0.0.1:$CURSOR_PORT")
                    while (cursorServerRunning && !serviceDestroyed) {
                        val client = try {
                            server.accept()
                        } catch (closed: Exception) {
                            if (cursorServerRunning && !serviceDestroyed) {
                                Log.w(TAG, "Cursor accept failed", closed)
                            }
                            break
                        }
                        client.tcpNoDelay = true
                        if (!cursorConnectionSlots.tryAcquire()) {
                            client.close()
                            continue
                        }
                        cursorClients.add(client)
                        try {
                            workerPool.execute { serveCursorClient(client) }
                        } catch (_: RejectedExecutionException) {
                            cursorClients.remove(client)
                            cursorConnectionSlots.release()
                            try { client.close() } catch (_: Exception) {}
                        }
                    }
                }
            } catch (failure: Exception) {
                if (cursorServerRunning && !serviceDestroyed) {
                    Log.e(TAG, "Custom cursor server failed", failure)
                }
            } finally {
                cursorServerRunning = false
                cursorServerStarting = false
                cursorServerSocket = null
                cursorClientSocket = null
                cursorClients.forEach { client -> try { client.close() } catch (_: Exception) {} }
                cursorClients.clear()
                executor?.shutdownNow()
                cursorExecutor = null
                if (!serviceDestroyed) {
                    mainHandler.postDelayed({
                        if (!serviceDestroyed && !cursorServerRunning) startCursorServer()
                    }, 1000)
                }
            }
        }, "waytouch-cursor-control").apply {
            isDaemon = true
            start()
        }
    }

    private fun enqueueCursorShow(line: String): Boolean {
        pendingCursorShow.set(line)
        if (cursorShowDrainScheduled.compareAndSet(false, true)) {
            if (!mainHandler.post { drainCursorShows() }) {
                cursorShowDrainScheduled.set(false)
                return false
            }
        }
        return true
    }

    private fun drainCursorShows() {
        while (!serviceDestroyed && cursorServerRunning) {
            val line = pendingCursorShow.getAndSet(null) ?: break
            try {
                if (!handleCursorCommand(line)) {
                    Log.w(TAG, "Coalesced cursor position was rejected")
                }
            } catch (failure: Exception) {
                Log.e(TAG, "Coalesced cursor position failed", failure)
            }
        }
        cursorShowDrainScheduled.set(false)
        // Close the race where a SHOW arrived after the final getAndSet(null)
        // but before the scheduled flag was cleared.
        if (pendingCursorShow.get() != null &&
            cursorShowDrainScheduled.compareAndSet(false, true)
        ) {
            if (!mainHandler.post { drainCursorShows() }) {
                cursorShowDrainScheduled.set(false)
            }
        }
    }

    private fun serveCursorClient(client: Socket) {
        var authenticated = false
        var ownsCursor = false
        try {
            client.use { socket ->
                // Authenticate first, but do not reserve the exclusive cursor stream
                // until the first command proves this is a controller, not a PING probe.
                socket.soTimeout = 750
                val token = ipcToken
                if (token == null || !authenticateCursorClient(socket, token)) {
                    Log.w(TAG, "Rejected unauthenticated cursor client")
                    return
                }
                authenticated = true
                val input = BufferedInputStream(socket.getInputStream(), 8 * 1024)
                val output = socket.getOutputStream()
                var line = readEditorBoundedLine(input, 256) ?: return

                // MainActivity and diagnostics use short-lived authenticated PING
                // requests. They must remain available while the real cursor owner
                // holds the persistent stream.
                if (line == "PING") {
                    output.write("PONG\n".toByteArray(Charsets.US_ASCII))
                    output.flush()
                    return
                }

                if (!cursorClientActive.compareAndSet(false, true)) {
                    Log.w(TAG, "Rejected extra authenticated cursor controller")
                    return
                }
                ownsCursor = true
                cursorClientSocket = socket
                socket.soTimeout = 0

                while (cursorServerRunning && !serviceDestroyed) {
                    if (line == "PING") {
                        output.write("PONG\n".toByteArray(Charsets.US_ASCII))
                        output.flush()
                    } else if (line.startsWith("SHOW ")) {
                        if (!enqueueCursorShow(line)) {
                            throw IllegalStateException("main looper rejected cursor position")
                        }
                        output.write("OK CURSOR\n".toByteArray(Charsets.US_ASCII))
                        output.flush()
                    } else {
                        val applied = CountDownLatch(1)
                        var commandFailure: Exception? = null
                        var commandSucceeded = false
                        val posted = mainHandler.post {
                            try {
                                if (serviceDestroyed || !cursorServerRunning) {
                                    throw IllegalStateException("cursor service is stopping")
                                }
                                commandSucceeded = handleCursorCommand(line)
                                if (!commandSucceeded) {
                                    throw IllegalStateException("cursor overlay command returned false")
                                }
                            } catch (failure: Exception) {
                                commandFailure = failure
                                Log.e(TAG, "Cursor command application failed", failure)
                            } finally {
                                applied.countDown()
                            }
                        }
                        if (!posted) {
                            throw IllegalStateException("main looper rejected cursor command")
                        }
                        if (!applied.await(100, TimeUnit.MILLISECONDS)) {
                            throw java.net.SocketTimeoutException(
                                "cursor visibility transition exceeded 100 ms"
                            )
                        }
                        commandFailure?.let { throw it }
                        if (!commandSucceeded) {
                            throw IllegalStateException("cursor command was not applied")
                        }
                        output.write("OK CURSOR\n".toByteArray(Charsets.US_ASCII))
                        output.flush()
                    }

                    line = readEditorBoundedLine(input, 256) ?: break
                }
            }
        } catch (failure: Exception) {
            if (!serviceDestroyed && cursorServerRunning) {
                Log.e(
                    TAG,
                    "Cursor client closed type=" + failure.javaClass.name +
                        " message=" + failure.message,
                    failure
                )
            }
        } finally {
            if (cursorClientSocket === client) cursorClientSocket = null
            if (ownsCursor) {
                cursorClientActive.set(false)
                if (authenticated && !serviceDestroyed && cursorServerRunning) {
                    mainHandler.post { if (!serviceDestroyed) removeCursor() }
                }
            }
            cursorClients.remove(client)
            cursorConnectionSlots.release()
        }
    }

    private val mainHandler = Handler(Looper.getMainLooper())
    private val pendingCursorShow = AtomicReference<String?>(null)
    private val cursorShowDrainScheduled = AtomicBoolean(false)

    private fun handleCursorCommand(line: String): Boolean {
        return when {
            line == "HIDE" || line == "OFF" -> {
                removeCursor()
                true
            }
            line.startsWith("SHOW ") -> {
                val parts = line.trim().split(CURSOR_FIELD_SEPARATOR)
                if (parts.size != 6) return false
                val x = parts[1].toFloatOrNull()?.takeIf { it.isFinite() }?.coerceIn(0f, 1f)
                    ?: return false
                val y = parts[2].toFloatOrNull()?.takeIf { it.isFinite() }?.coerceIn(0f, 1f)
                    ?: return false
                val style = parts[3]
                if (style !in CURSOR_STYLES) return false
                val size = parts[4].toFloatOrNull()?.takeIf { it.isFinite() }?.coerceIn(12f, 64f)
                    ?: return false
                val color = try {
                    Color.parseColor(parts[5])
                } catch (_: Exception) {
                    return false
                }
                showCursor(x, y, style, size, color)
            }
            else -> false
        }
    }

    /**
     * Small non-touchable cursor window. A full-screen overlay is unnecessary
     * for a cursor and can interfere with game touch delivery / overlay checks.
     * Only this bounded rectangle is placed above the foreground application.
     */
    @Suppress("DEPRECATION")
    private fun showCursor(x: Float, y: Float, style: String, size: Float, color: Int): Boolean {
        val wm = windowManager ?: return false
        val view = cursorView ?: CursorOverlayView().also { cursorView = it }
        val displaySize = android.graphics.Point().also { wm.defaultDisplay.getRealSize(it) }
        val density = resources.displayMetrics.density
        // The pointer style extends farther than the crosshair; reserve enough
        // local canvas space for every supported cursor size plus its halo.
        val sidePx = (max(64f, size * 2f + 16f) * density + 0.5f)
            .toInt()
            .coerceAtLeast(48)
        val windowX = (x.coerceIn(0f, 1f) * (displaySize.x - 1).coerceAtLeast(1) - sidePx / 2f).toInt()
        val windowY = (y.coerceIn(0f, 1f) * (displaySize.y - 1).coerceAtLeast(1) - sidePx / 2f).toInt()

        if (!view.isAttachedToWindow) {
            val params = WindowManager.LayoutParams(
                sidePx,
                sidePx,
                WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                    WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                    WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or
                    WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
                PixelFormat.TRANSLUCENT
            ).apply {
                gravity = Gravity.TOP or Gravity.START
                this.x = windowX
                this.y = windowY
                alpha = 0.8f
                if (Build.VERSION.SDK_INT >= 28) {
                    layoutInDisplayCutoutMode =
                        WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
                }
                title = "WayTouch Cursor"
            }

            try {
                wm.addView(view, params)
                cursorWindowParams = params
            } catch (failure: Exception) {
                Log.e(TAG, "Cursor overlay attach failed", failure)
                if (cursorView === view) cursorView = null
                cursorWindowParams = null
                return false
            }
        } else {
            val params = cursorWindowParams ?: return false
            val layoutChanged = params.x != windowX || params.y != windowY ||
                params.width != sidePx || params.height != sidePx
            if (layoutChanged) {
                params.x = windowX
                params.y = windowY
                params.width = sidePx
                params.height = sidePx
                try {
                    wm.updateViewLayout(view, params)
                } catch (failure: Exception) {
                    Log.e(TAG, "Cursor overlay position update failed", failure)
                    removeCursorImmediately()
                    return false
                }
            }
        }

        // The view is only cursor-sized, so its local drawing target is always
        // the center; only the small WindowManager window follows screen x/y.
        view.setTarget(0.5f, 0.5f, style, size, color)
        return true
    }

    /** Fade out, then detach. A new SHOW during the fade cancels the removal. */
    private fun removeCursor() {
        cursorView?.hide()
    }

    /** Service shutdown is different from a protocol HIDE: do not leave an animation alive. */
    private fun removeCursorImmediately() {
        val view = cursorView
        cursorView = null
        cursorWindowParams = null
        view?.detachImmediately()
    }

    private inner class CursorOverlayView : View(this@EditorService) {
        private val corePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            strokeCap = Paint.Cap.ROUND
            strokeJoin = Paint.Join.ROUND
        }
        private val haloPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            strokeCap = Paint.Cap.ROUND
            strokeJoin = Paint.Join.ROUND
        }
        private val shadowPaint = Paint(Paint.ANTI_ALIAS_FLAG)
        private val arrowPath = android.graphics.Path()

        private var style = "crosshair"
        private var sizeDp = 28f
        private var color = Color.rgb(255, 49, 88)
        private var targetX = 0.5f
        private var targetY = 0.5f
        private var x = -1f
        private var y = -1f
        private var alphaFraction = 0f
        private var visible = false
        private var lastFrameUptime = 0L
        private var cachedRadius = -1f
        private var shadowGradient: RadialGradient? = null
        private var arrowScale = -1f

        private val density = resources.displayMetrics.density

        fun setTarget(nx: Float, ny: Float, nextStyle: String, nextSize: Float, nextColor: Int) {
            targetX = nx.coerceIn(0f, 1f)
            targetY = ny.coerceIn(0f, 1f)
            style = nextStyle
            sizeDp = nextSize
            color = nextColor
            visible = true
            postInvalidateOnAnimation()
        }

        fun hide() {
            if (!visible) return
            visible = false
            postInvalidateOnAnimation()
        }

        fun detachImmediately() {
            visible = false
            alphaFraction = 0f
            try {
                if (isAttachedToWindow) windowManager?.removeViewImmediate(this)
            } catch (failure: Exception) {
                Log.w(TAG, "Cursor overlay detach during service shutdown failed", failure)
            }
        }

        override fun onDetachedFromWindow() {
            super.onDetachedFromWindow()
            x = -1f
            y = -1f
            lastFrameUptime = 0L
        }

        override fun onDraw(canvas: Canvas) {
            super.onDraw(canvas)
            if (width <= 0 || height <= 0) return

            if (x < 0f || y < 0f) {
                if (!visible) {
                    detachAfterFade()
                    return
                }
                x = targetX * width
                y = targetY * height
            }

            val now = SystemClock.uptimeMillis()
            val deltaMs = if (lastFrameUptime == 0L) {
                16f
            } else {
                (now - lastFrameUptime).coerceIn(1L, 32L).toFloat()
            }
            lastFrameUptime = now

            // Match the acknowledged Android pointer position exactly; animate
            // opacity only. Positional smoothing causes aim/click drift.
            val alphaFactor = (1.0 - kotlin.math.exp(-deltaMs / 90f)).toFloat()
            val nextX = targetX * width
            val nextY = targetY * height
            val targetAlpha = if (visible) 1f else 0f

            val oldX = x
            val oldY = y
            x = nextX
            y = nextY
            alphaFraction += (targetAlpha - alphaFraction) * alphaFactor

            drawCursor(canvas)

            val positionSettled = abs(nextX - x) < 0.3f && abs(nextY - y) < 0.3f
            val alphaSettled = abs(targetAlpha - alphaFraction) < 0.012f

            if (!visible && alphaFraction <= 0.012f) {
                alphaFraction = 0f
                lastFrameUptime = 0L
                post { detachAfterFade() }
                return
            }

            if (visible && positionSettled && alphaSettled) {
                x = nextX
                y = nextY
                alphaFraction = 1f
                lastFrameUptime = 0L
                return
            }

            val radius = cursorRadius() + density * 7f
            val left = (minOf(x, oldX) - radius).toInt().coerceAtLeast(0)
            val top = (minOf(y, oldY) - radius).toInt().coerceAtLeast(0)
            val right = (maxOf(x, oldX) + radius).toInt().coerceAtMost(width)
            val bottom = (maxOf(y, oldY) + radius).toInt().coerceAtMost(height)
            postInvalidateOnAnimation(left, top, right, bottom)
        }

        private fun detachAfterFade() {
            if (visible || alphaFraction > 0.012f || !isAttachedToWindow) return
            try {
                windowManager?.removeView(this)
            } catch (failure: Exception) {
                Log.w(TAG, "Cursor overlay detach failed", failure)
            }
            if (cursorView === this) {
                cursorView = null
                cursorWindowParams = null
            }
        }

        private fun cursorRadius(): Float = sizeDp * density * 0.5f

        private fun drawCursor(canvas: Canvas) {
            if (alphaFraction <= 0.01f) return
            val radius = cursorRadius()
            val alpha = (alphaFraction * 255f).toInt().coerceIn(0, 255)

            if (shadowGradient == null || cachedRadius != radius) {
                cachedRadius = radius
                shadowGradient = RadialGradient(
                    0f, 0f, radius * 1.35f,
                    intArrayOf(0x44000000, 0x22000000, 0x00000000),
                    floatArrayOf(0f, 0.58f, 1f),
                    android.graphics.Shader.TileMode.CLAMP
                )
            }
            shadowPaint.shader = shadowGradient
            shadowPaint.alpha = alpha
            canvas.save()
            canvas.translate(x, y + density * 1.5f)
            canvas.drawCircle(0f, 0f, radius * 1.25f, shadowPaint)
            canvas.restore()

            corePaint.color = color
            corePaint.alpha = alpha
            haloPaint.color = Color.WHITE
            haloPaint.alpha = (alphaFraction * 238f).toInt().coerceIn(0, 255)

            when (style) {
                "pointer" -> drawPointer(canvas, radius)
                "dot" -> drawDot(canvas, radius)
                else -> drawCrosshair(canvas, radius)
            }
        }

        private fun drawCrosshair(canvas: Canvas, radius: Float) {
            val coreWidth = max(1.5f, density * 1.6f)
            drawCrosshairPass(canvas, radius, haloPaint, coreWidth + density * 2.2f)
            drawCrosshairPass(canvas, radius, corePaint, coreWidth)

            haloPaint.style = Paint.Style.FILL
            corePaint.style = Paint.Style.FILL
            canvas.drawCircle(x, y, density * 2.8f, haloPaint)
            canvas.drawCircle(x, y, density * 1.5f, corePaint)
        }

        private fun drawCrosshairPass(canvas: Canvas, radius: Float, paint: Paint, stroke: Float) {
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = stroke
            val ring = radius * 0.57f
            val inner = radius * 0.30f
            val outer = radius * 0.98f
            canvas.drawCircle(x, y, ring, paint)
            canvas.drawLine(x - outer, y, x - inner, y, paint)
            canvas.drawLine(x + inner, y, x + outer, y, paint)
            canvas.drawLine(x, y - outer, x, y - inner, paint)
            canvas.drawLine(x, y + inner, x, y + outer, paint)
        }

        private fun drawPointer(canvas: Canvas, radius: Float) {
            val scale = radius / 16f
            if (arrowScale != scale) {
                arrowScale = scale
                arrowPath.reset()
                arrowPath.moveTo(0f, 0f)
                arrowPath.lineTo(0f, 24f * scale)
                arrowPath.lineTo(6f * scale, 18f * scale)
                arrowPath.lineTo(10f * scale, 27f * scale)
                arrowPath.lineTo(14f * scale, 25f * scale)
                arrowPath.lineTo(10f * scale, 16f * scale)
                arrowPath.lineTo(18f * scale, 16f * scale)
                arrowPath.close()
            }

            canvas.save()
            canvas.translate(x, y)
            haloPaint.style = Paint.Style.FILL_AND_STROKE
            haloPaint.strokeWidth = density * 2.2f
            corePaint.style = Paint.Style.FILL
            canvas.drawPath(arrowPath, haloPaint)
            canvas.drawPath(arrowPath, corePaint)
            canvas.restore()
        }

        private fun drawDot(canvas: Canvas, radius: Float) {
            haloPaint.style = Paint.Style.FILL
            corePaint.style = Paint.Style.FILL
            canvas.drawCircle(x, y, radius * 0.31f + density * 1.4f, haloPaint)
            canvas.drawCircle(x, y, radius * 0.31f, corePaint)

            haloPaint.style = Paint.Style.STROKE
            haloPaint.strokeWidth = density * 3.2f
            corePaint.style = Paint.Style.STROKE
            corePaint.strokeWidth = density * 1.5f
            canvas.drawCircle(x, y, radius * 0.76f, haloPaint)
            canvas.drawCircle(x, y, radius * 0.76f, corePaint)
        }
    }

    private fun servicePending(action: String, request: Int): PendingIntent {
        val intent = Intent(this, EditorService::class.java).setAction(action)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            (if (Build.VERSION.SDK_INT >= 23) PendingIntent.FLAG_IMMUTABLE else 0)
        return PendingIntent.getService(this, request, intent, flags)
    }

    private fun buildNotification(): Notification {
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            (if (Build.VERSION.SDK_INT >= 23) PendingIntent.FLAG_IMMUTABLE else 0)
        val content = PendingIntent.getActivity(
            this, 10, Intent(this, MainActivity::class.java), flags
        )
        val builder = if (Build.VERSION.SDK_INT >= 26) {
            Notification.Builder(this, CHANNEL_ID)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(this)
        }
        return builder
            .setSmallIcon(R.drawable.ic_waytouch_notification)
            .setContentTitle("WayTouch · تنظیم کنترل‌ها")
            .setContentText(if (overlayAttached) "پنل ویرایش باز است" else "ویرایش چیدمان بازی")
            .setContentIntent(content)
            .setOngoing(true)
            .setCategory(Notification.CATEGORY_SERVICE)
            .addAction(Notification.Action.Builder(
                android.R.drawable.ic_menu_edit, "باز کردن پنل", servicePending(ACTION_OPEN, 1)
            ).build())
            .addAction(Notification.Action.Builder(
                android.R.drawable.ic_menu_close_clear_cancel, "بستن پنل", servicePending(ACTION_CLOSE, 2)
            ).build())
            .addAction(Notification.Action.Builder(
                android.R.drawable.ic_menu_close_clear_cancel, "خاموش کردن", servicePending(ACTION_STOP, 3)
            ).build())
            .build()
    }

    private fun refreshNotification() {
        // Re-post via startForeground so Android refreshes the active service
        // notification's content after OPEN/CLOSE state transitions.
        startForeground(NOTIFICATION_ID, buildNotification())
        Log.i(TAG, "Notification refreshed editorOpen=" + overlayAttached)
    }

    private fun showEditor() {
        Log.i(
            TAG,
            "Open requested; attached=" + overlayAttached +
                " viewAttached=" + (editorView?.isAttachedToWindow == true)
        )
        if (!Settings.canDrawOverlays(this)) {
            Toast.makeText(applicationContext, "اجازهٔ نمایش روی برنامه‌ها فعال نیست.", Toast.LENGTH_LONG).show()
            startActivity(Intent(this, MainActivity::class.java).addFlags(
                Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
            ))
            return
        }
        if (overlayAttached && editorView?.isAttachedToWindow == true) return
        if (loadingLayout) {
            Log.i(TAG, "Layout GET already running; duplicate OPEN ignored")
            return
        }

        // Socket work must not run on Android's main thread. Keep the service/UI
        // responsive while the revisioned GET is performed on a worker.
        loadingLayout = true
        val requestGeneration = ++editorRequestGeneration
        Thread({
            val loaded = fetchLayout()
            Handler(Looper.getMainLooper()).post {
                if (requestGeneration != editorRequestGeneration) {
                    Log.i(TAG, "Discarding stale layout GET result")
                    return@post
                }
                loadingLayout = false
                if (loaded == null) {
                    Toast.makeText(
                        applicationContext,
                        "سرور چیدمان در دسترس نیست یا پروفایل هنوز همگام نشده است؛ ابتدا WayTouch را در لینوکس راه‌اندازی کنید.",
                        Toast.LENGTH_LONG
                    ).show()
                    refreshNotification()
                    return@post
                }
                attachEditor(loaded)
            }
        }, "waytouch-layout-fetch").start()
    }

    private fun createEditorLayoutParams(): WindowManager.LayoutParams =
        WindowManager.LayoutParams(
            WindowManager.LayoutParams.MATCH_PARENT,
            WindowManager.LayoutParams.MATCH_PARENT,
            WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY,
            WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS or
                WindowManager.LayoutParams.FLAG_FULLSCREEN or
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE,
            PixelFormat.TRANSLUCENT
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            x = 0
            y = 0
            if (Build.VERSION.SDK_INT >= 28) {
                layoutInDisplayCutoutMode =
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
            }
            title = "WayTouch Fullscreen Layout Editor"
        }

    private fun attachEditor(loaded: LoadedLayout) {
        // Recover if Android detached the overlay while switching secure system surfaces.
        if (overlayAttached) {
            editorView = null
            overlayAttached = false
        }
        layoutRevision = loaded.revision
        val view = LayoutEditorView(loaded.layout)

        val params = createEditorLayoutParams()

        try {
            windowManager?.addView(view, params)
            editorView = view
            overlayAttached = true
            Log.i(TAG, "Overlay window attached: " + view.width + "x" + view.height)
            refreshNotification()
        } catch (failure: Exception) {
            Log.e(TAG, "Overlay attach failed", failure)
            Toast.makeText(applicationContext, "نمایش پنل ممکن نشد: " + failure.message, Toast.LENGTH_LONG).show()
            refreshNotification()
        }
    }

    private fun testEditorInGame(view: LayoutEditorView) {
        if (testingGame || editorView !== view || !view.isAttachedToWindow) return
        val wm = windowManager ?: return
        try {
            wm.removeView(view)
        } catch (failure: Exception) {
            Log.e(TAG, "Could not temporarily hide editor for game test", failure)
            Toast.makeText(applicationContext, "پنهان‌کردن ویرایشگر ممکن نشد.", Toast.LENGTH_LONG).show()
            return
        }

        overlayAttached = false
        testingGame = true
        Toast.makeText(
            applicationContext,
            "حالت آزمایش فعال شد؛ پس از ۵ ثانیه ویرایشگر باز می‌شود.",
            Toast.LENGTH_LONG
        ).show()

        val restore = Runnable {
            testRestoreRunnable = null
            if (serviceDestroyed || !testingGame || editorView !== view) return@Runnable
            testingGame = false
            try {
                wm.addView(view, createEditorLayoutParams())
                overlayAttached = true
                refreshNotification()
                Toast.makeText(applicationContext, "ویرایشگر دوباره باز شد.", Toast.LENGTH_SHORT).show()
            } catch (failure: Exception) {
                overlayAttached = false
                editorView = null
                Log.e(TAG, "Could not restore editor after game test", failure)
                Toast.makeText(
                    applicationContext,
                    "بازگرداندن ویرایشگر ناموفق بود: " + failure.message,
                    Toast.LENGTH_LONG
                ).show()
                refreshNotification()
            }
        }
        testRestoreRunnable = restore
        mainHandler.postDelayed(restore, 5_000L)
    }

    private fun hideEditor(updateNotification: Boolean = true) {
        // Invalidate any outstanding background GET so a delayed result cannot
        // reopen the overlay after the user closed it from the notification.
        editorRequestGeneration += 1L
        loadingLayout = false
        testRestoreRunnable?.let(mainHandler::removeCallbacks)
        testRestoreRunnable = null
        testingGame = false
        val view = editorView
        if (view != null && overlayAttached) {
            try {
                windowManager?.removeView(view)
            } catch (_: Exception) {
            }
        }
        Log.i(TAG, "Overlay closed")
        editorView = null
        overlayAttached = false
        if (updateNotification) refreshNotification()
    }

    private fun fetchLayout(): LoadedLayout? {
        return try {
            Socket().use { socket ->
                socket.connect(InetSocketAddress("127.0.0.1", CONFIG_PORT), 900)
                socket.soTimeout = 900
                val writer = socket.getOutputStream().bufferedWriter(Charsets.UTF_8)
                val input = BufferedInputStream(socket.getInputStream(), 16 * 1024)
                val token = ipcToken ?: return null
                writer.write("AUTH " + token + "\n")
                writer.flush()
                if (readEditorBoundedLine(input, 80) != "OK AUTH") return null
                writer.write("GET\n")
                writer.flush()
                val line = readEditorBoundedLine(input, 1024 * 1024) ?: return null
                if (line == "NONE" || line.startsWith("ERR")) return null
                val envelope = JSONObject(line)
                val revision = envelope.getLong("revision")
                require(revision >= 0L) { "invalid layout revision" }
                LoadedLayout(revision, EditorLayout.fromJson(envelope.getJSONObject("layout").toString()))
            }
        } catch (failure: Exception) {
            Log.e(TAG, "Layout GET failed", failure)
            null
        }
    }

    private fun saveLayout(layout: EditorLayout): Long? {
        return try {
            Socket().use { socket ->
                socket.connect(InetSocketAddress("127.0.0.1", CONFIG_PORT), 1200)
                socket.soTimeout = 1200
                val writer = socket.getOutputStream().bufferedWriter(Charsets.UTF_8)
                val input = BufferedInputStream(socket.getInputStream(), 16 * 1024)
                val token = ipcToken ?: return null
                writer.write("AUTH " + token + "\n")
                writer.flush()
                if (readEditorBoundedLine(input, 80) != "OK AUTH") return null
                writer.write("SET " + layoutRevision + " " + layout.toJson() + "\n")
                writer.flush()
                val reply = readEditorBoundedLine(input, 1024 * 1024).orEmpty()
                Log.i(TAG, "Layout SET reply=" + reply)
                if (reply.startsWith("OK ")) reply.removePrefix("OK ").toLongOrNull() else null
            }
        } catch (failure: Exception) {
            Log.e(TAG, "Layout SET failed", failure)
            null
        }
    }

    private fun finishEditor(save: Boolean, layout: EditorLayout) {
        if (!save) {
            hideEditor()
            return
        }
        if (saveInProgress) return
        saveInProgress = true
        // Android forbids socket I/O on the UI thread. Keep drawing responsive while
        // the small loopback SET request is performed on a worker thread.
        Thread({
            val savedRevision = saveLayout(layout)
            Handler(Looper.getMainLooper()).post {
                saveInProgress = false
                if (savedRevision == null) {
                    Toast.makeText(
                        applicationContext,
                        "ذخیره انجام نشد؛ ممکن است چیدمان در سمت لینوکس تغییر کرده باشد. پنل را دوباره باز کنید.",
                        Toast.LENGTH_LONG
                    ).show()
                    return@post
                }
                layoutRevision = savedRevision
                Toast.makeText(applicationContext, "چیدمان ذخیره و زنده اعمال شد.", Toast.LENGTH_SHORT).show()
                hideEditor()
            }
        }, "waytouch-layout-save").start()
    }

    private inner class LayoutEditorView(private val layout: EditorLayout) : View(this@EditorService) {
        init {
            isClickable = true
            layoutDirection = View.LAYOUT_DIRECTION_RTL
            systemUiVisibility = SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or
                SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or
                SYSTEM_UI_FLAG_LAYOUT_STABLE
        }

        private val density = resources.displayMetrics.density
        private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
        private val snapGuidePathEffect = DashPathEffect(
            floatArrayOf(dp(6f), dp(5f)), 0f
        )
        private val selectionRingPathEffect = DashPathEffect(
            floatArrayOf(dp(5f), dp(4f)), 0f
        )
        private var preview = RectF()
        private var basePreview = RectF()
        private var mapViewport = RectF()
        private var zoom = 1f
        private var panX = 0f
        private var panY = 0f
        private var panning = false
        private var lastPanX = 0f
        private var lastPanY = 0f
        private var twoFingerFocusX = 0f
        private var twoFingerFocusY = 0f
        private var twoFingerPanning = false
        private val scaleDetector = ScaleGestureDetector(
            this@EditorService,
            object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
                override fun onScale(detector: ScaleGestureDetector): Boolean {
                    val next = (zoom * detector.scaleFactor).coerceIn(1f, 5f)
                    if (next != zoom) {
                        zoom = next
                        if (zoom <= 1f) {
                            panX = 0f
                            panY = 0f
                        }
                        rebuildPreview()
                        selectedName = "بزرگ‌نمایی " + String.format(java.util.Locale.US, "%.2f", zoom)
                        invalidate()
                    }
                    return true
                }
            }
        )
        private var zoomInRect = RectF()
        private var zoomOutRect = RectF()
        private var zoomResetRect = RectF()
        private var saveRect = RectF()
        private var cancelRect = RectF()
        private var undoRect = RectF()
        private var redoRect = RectF()
        private var testRect = RectF()
        private var renameRect = RectF()
        private var selectionPanelRect = RectF()
        private var selectionBindingRect = RectF()
        private var selectionRenameRect = RectF()
        private var selectionRemoveRect = RectF()
        private var nudgeStepRect = RectF()
        private val nudgeSteps = floatArrayOf(0.001f, 0.005f, 0.01f, 0.05f)
        private val nudgeStepLabels = arrayOf("۰٫۱٪", "۰٫۵٪", "۱٪", "۵٪")
        private var nudgeStepIndex = 0
        private val originalLayoutJson = layout.toJson()
        private val undoHistory = mutableListOf<String>()
        private val redoHistory = mutableListOf<String>()
        private var addKeyRect = RectF()
        private var addMouseRect = RectF()
        private var editRect = RectF()
        private var removeRect = RectF()
        private var moveRect = RectF()
        private var gridRect = RectF()
        private var nudgeUpRect = RectF()
        private var nudgeDownRect = RectF()
        private var nudgeLeftRect = RectF()
        private var nudgeRightRect = RectF()
        private var gridEnabled = true
        private var pickerCloseRect = RectF()
        private val pickerRects = mutableListOf<RectF>()
        private var pickerOptions = emptyList<TriggerOption>()
        private var pickerPage = 0
        private val pickerPageSize = 20
        private var pickerPreviousRect = RectF()
        private var pickerNextRect = RectF()
        private var pickerSearchRect = RectF()
        private var pickerAllOptions = emptyList<TriggerOption>()
        private var movementKeysRect = RectF()
        private var triggerPickerOpen = false
        private var selected = Int.MIN_VALUE
        private var selectedName = "WASD"
        private var dragging = false
        private var dragHapticSent = false
        private var wasSnapped = false
        private var snapGuideX: Float? = null
        private var snapGuideY: Float? = null
        private val displaySizeCache = Point()
        private val viewLocationOnScreen = IntArray(2)
        private var lastCoordinateSpaceLog: String? = null
        private var cachedDisplayWidth = 0
        private var cachedDisplayHeight = 0

        private val keyOptions = buildList {
            addAll(listOf(
            TriggerOption("A", "key", 30), TriggerOption("B", "key", 48),
            TriggerOption("C", "key", 46), TriggerOption("D", "key", 32),
            TriggerOption("E", "key", 18), TriggerOption("F", "key", 33),
            TriggerOption("G", "key", 34), TriggerOption("H", "key", 35),
            TriggerOption("I", "key", 23), TriggerOption("J", "key", 36),
            TriggerOption("K", "key", 37), TriggerOption("L", "key", 38),
            TriggerOption("M", "key", 50), TriggerOption("N", "key", 49),
            TriggerOption("O", "key", 24), TriggerOption("P", "key", 25),
            TriggerOption("Q", "key", 16), TriggerOption("R", "key", 19),
            TriggerOption("S", "key", 31), TriggerOption("T", "key", 20),
            TriggerOption("U", "key", 22), TriggerOption("V", "key", 47),
            TriggerOption("W", "key", 17), TriggerOption("X", "key", 45),
            TriggerOption("Y", "key", 21), TriggerOption("Z", "key", 44),
            TriggerOption("0", "key", 11), TriggerOption("1", "key", 2),
            TriggerOption("2", "key", 3), TriggerOption("3", "key", 4),
            TriggerOption("4", "key", 5), TriggerOption("5", "key", 6),
            TriggerOption("6", "key", 7), TriggerOption("7", "key", 8),
            TriggerOption("8", "key", 9), TriggerOption("9", "key", 10),
            TriggerOption("F4", "key", 62),
            TriggerOption("F5", "key", 63), TriggerOption("F6", "key", 64),
            TriggerOption("F7", "key", 65), TriggerOption("F8", "key", 66),
            TriggerOption("F9", "key", 67), TriggerOption("F10", "key", 68),
            TriggerOption("F11", "key", 87), TriggerOption("F12", "key", 88),
            TriggerOption("SPACE", "key", 57), TriggerOption("ENTER", "key", 28),
            TriggerOption("BACKSPACE", "key", 14), TriggerOption("TAB", "key", 15),
            TriggerOption("ESC", "key", 1), TriggerOption("SHIFT", "key", 42),
            TriggerOption("CTRL", "key", 29), TriggerOption("ALT", "key", 56),
            TriggerOption("CAPS", "key", 58), TriggerOption("HOME", "key", 102),
            TriggerOption("END", "key", 107), TriggerOption("PGUP", "key", 104),
            TriggerOption("PGDN", "key", 109), TriggerOption("INS", "key", 110),
                TriggerOption("DEL", "key", 111)
            ))
            val knownCodes = map { it.code }.toSet()
        // Keep common keys human-readable, but expose every other Linux EV_KEY
        // code too so the number of configurable actions is not tied to this list.
            for (code in 1..767) {
                if (code !in knownCodes && code !in 272..287) {
                    add(TriggerOption("KEY_$code", "key", code))
                }
            }
        }
        private val mouseOptions = (272..287).map { code ->
            val label = when (code) {
                272 -> "LMB"
                273 -> "RMB"
                274 -> "MMB"
                275 -> "SIDE"
                276 -> "EXTRA"
                else -> "BTN_${code - 271}"
            }
            TriggerOption(label, "mouse", code)
        }

        private fun dp(value: Float) = value * density

        private fun pushUndo() {
            val snapshot = layout.toJson()
            if (undoHistory.lastOrNull() == snapshot) return
            undoHistory.add(snapshot)
            if (undoHistory.size > 50) undoHistory.removeAt(0)
            redoHistory.clear()
        }

        private fun restoreLayoutSnapshot(snapshot: String, label: String) {
            val restored = EditorLayout.fromJson(snapshot)
            layout.centerX = restored.centerX
            layout.centerY = restored.centerY
            layout.radius = restored.radius
            layout.upKey = restored.upKey
            layout.leftKey = restored.leftKey
            layout.downKey = restored.downKey
            layout.rightKey = restored.rightKey
            layout.buttons.clear()
            layout.buttons.addAll(restored.buttons)
            selected = Int.MIN_VALUE
            selectedName = label
            snapGuideX = null
            snapGuideY = null
            invalidate()
        }

        private fun undoLastChange() {
            if (undoHistory.isEmpty()) {
                Toast.makeText(applicationContext, "تغییری برای بازگردانی وجود ندارد.", Toast.LENGTH_SHORT).show()
                return
            }
            redoHistory.add(layout.toJson())
            val snapshot = undoHistory.removeAt(undoHistory.lastIndex)
            restoreLayoutSnapshot(snapshot, "بازگشت")
        }

        private fun redoLastChange() {
            if (redoHistory.isEmpty()) {
                Toast.makeText(applicationContext, "تغییری برای انجام دوباره وجود ندارد.", Toast.LENGTH_SHORT).show()
                return
            }
            undoHistory.add(layout.toJson())
            if (undoHistory.size > 50) undoHistory.removeAt(0)
            val snapshot = redoHistory.removeAt(redoHistory.lastIndex)
            restoreLayoutSnapshot(snapshot, "انجام دوباره")
        }

        private fun confirmCancel() {
            if (layout.toJson() == originalLayoutJson) {
                finishEditor(false, layout)
                return
            }
            val dialog = AlertDialog.Builder(this@EditorService)
                .setTitle("تغییرات ذخیره‌نشده")
                .setMessage("تغییراتی که ذخیره نکرده‌اید حذف شوند؟")
                .setNegativeButton("ادامهٔ ویرایش", null)
                .setPositiveButton("دور انداختن تغییرات") { _, _ ->
                    finishEditor(false, layout)
                }
                .create()
            dialog.setCanceledOnTouchOutside(true)
            dialog.window?.setType(WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY)
            dialog.show()
        }

        private fun renameSelectedControl() {
            if (selected !in layout.buttons.indices) {
                Toast.makeText(applicationContext, "ابتدا یک دکمه را انتخاب کنید.", Toast.LENGTH_SHORT).show()
                return
            }
            val button = layout.buttons[selected]
            val editor = android.widget.EditText(this@EditorService).apply {
                setSingleLine(true)
                setText(button.name)
                selectAll()
                hint = "نام کنترل"
            }
            AlertDialog.Builder(this@EditorService)
                .setTitle("نام کنترل")
                .setView(editor)
                .setNegativeButton("انصراف", null)
                .setPositiveButton("اعمال") { _, _ ->
                    val name = editor.text.toString().trim()
                    val duplicate = layout.buttons.any { it !== button && it.name.equals(name, ignoreCase = true) }
                    if (name.isBlank() || name.codePointCount(0, name.length) > 40 || duplicate) {
                        Toast.makeText(
                            applicationContext,
                            "نام باید یکتا و حداکثر ۴۰ نویسه باشد.",
                            Toast.LENGTH_LONG
                        ).show()
                    } else if (name != button.name) {
                        pushUndo()
                        button.name = name
                        selectedName = name
                        invalidate()
                    }
                }
                .create()
                .also {
                    it.window?.setType(WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY)
                    it.show()
                }
        }

        override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
            super.onSizeChanged(w, h, oldw, oldh)
            @Suppress("DEPRECATION")
            windowManager?.defaultDisplay?.getRealSize(displaySizeCache)
            cachedDisplayWidth = displaySizeCache.x.takeIf { it > 0 }
                ?: resources.displayMetrics.widthPixels
            cachedDisplayHeight = displaySizeCache.y.takeIf { it > 0 }
                ?: resources.displayMetrics.heightPixels
            clampPan()
            Log.i(
                TAG,
                "Fullscreen editor size=" + w + "x" + h +
                    " physical=" + cachedDisplayWidth + "x" + cachedDisplayHeight +
                    " density=" + density
            )
        }

        override fun onDraw(canvas: Canvas) {
            // Keep the game visible beneath the editor while catching touches for positioning.
            canvas.drawColor(Color.argb(132, 4, 9, 16))
            val headerPanel = RectF(dp(12f), dp(12f), width - dp(12f), dp(82f))

            var toolbarRight = width - dp(22f)
            val toolbarWidth = dp(50f)
            val toolbarGap = dp(3f)
            fun toolbarRect(): RectF {
                val rect = RectF(toolbarRight - toolbarWidth, dp(18f), toolbarRight, dp(66f))
                toolbarRight -= toolbarWidth + toolbarGap
                return rect
            }
            addKeyRect = toolbarRect()
            addMouseRect = toolbarRect()
            editRect = toolbarRect()
            removeRect = toolbarRect()
            moveRect = toolbarRect()
            gridRect = toolbarRect()
            zoomInRect = toolbarRect()
            zoomOutRect = toolbarRect()
            zoomResetRect = toolbarRect()
            renameRect = toolbarRect()
            movementKeysRect = toolbarRect()
            val selectionVisible = selected != Int.MIN_VALUE && !triggerPickerOpen
            val displayWidth = cachedDisplayWidth.takeIf { it > 0 }
                ?: resources.displayMetrics.widthPixels
            val displayHeight = cachedDisplayHeight.takeIf { it > 0 }
                ?: resources.displayMetrics.heightPixels

            // Layout coordinates are injected from physical display origin (0, 0).
            // Convert that display rectangle into this overlay View's local space;
            // do not fit it into the smaller area left between the editor toolbars.
            getLocationOnScreen(viewLocationOnScreen)
            val screenLeft = -viewLocationOnScreen[0].toFloat()
            val screenTop = -viewLocationOnScreen[1].toFloat()
            val screenRight = screenLeft + (displayWidth - 1).coerceAtLeast(1).toFloat()
            val screenBottom = screenTop + (displayHeight - 1).coerceAtLeast(1).toFloat()
            mapViewport = RectF(screenLeft, screenTop, screenRight, screenBottom)
            basePreview = RectF(mapViewport)
            val coordinateSpaceLog =
                "Layout coordinate space view=${width}x${height} " +
                    "origin=${viewLocationOnScreen[0]},${viewLocationOnScreen[1]} " +
                    "display=${displayWidth}x${displayHeight} bounds=${mapViewport.toShortString()}"
            if (coordinateSpaceLog != lastCoordinateSpaceLog) {
                lastCoordinateSpaceLog = coordinateSpaceLog
                Log.i(TAG, coordinateSpaceLog)
            }
            rebuildPreview()
            canvas.save()
            canvas.clipRect(mapViewport)

            paint.color = Color.argb(22, 14, 29, 43)
            canvas.drawRect(preview, paint)
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = dp(1f)
            paint.color = Color.argb(110, 69, 100, 125)
            canvas.drawRect(preview, paint)
            if (gridEnabled) {
                paint.style = Paint.Style.STROKE
                paint.strokeWidth = dp(0.7f)
                paint.color = Color.argb(30, 120, 170, 205)
                for (i in 1..9) {
                    val gx = preview.left + preview.width() * i / 10f
                    val gy = preview.top + preview.height() * i / 10f
                    canvas.drawLine(gx, preview.top, gx, preview.bottom, paint)
                    canvas.drawLine(preview.left, gy, preview.right, gy, paint)
                }
            }
            paint.style = Paint.Style.FILL

            canvas.restore()

            // Draw editor chrome above the full-screen controls so edge controls
            // stay readable while every marker remains positioned on its real target.
            paint.style = Paint.Style.FILL
            paint.color = WayTouchDesign.PANEL
            canvas.drawRoundRect(headerPanel, dp(18f), dp(18f), paint)
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = dp(1f)
            paint.color = WayTouchDesign.STROKE
            canvas.drawRoundRect(headerPanel, dp(18f), dp(18f), paint)
            paint.style = Paint.Style.FILL
            paint.color = WayTouchDesign.TEXT
            paint.typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            paint.textSize = dp(19f)
            canvas.drawText("WayTouch · ویرایش چیدمان", dp(28f), dp(43f), paint)
            paint.color = WayTouchDesign.TEXT_MUTED
            paint.typeface = Typeface.DEFAULT
            paint.textSize = dp(11f)
            canvas.drawText(
                "صفحه ${displayWidth}×${displayHeight} · برای جابه‌جایی بکشید · " +
                    "بزرگ‌نمایی با دو انگشت · " + selectedName,
                dp(28f), dp(64f), paint
            )

            drawToolbarButton(canvas, addKeyRect, "+کلید", Color.rgb(37, 117, 156))
            drawToolbarButton(canvas, addMouseRect, "+ماوس", Color.rgb(36, 126, 108))
            drawToolbarButton(canvas, editRect, "اتصال", Color.rgb(87, 101, 142))
            drawToolbarButton(canvas, removeRect, "حذف", Color.rgb(143, 72, 76))
            drawToolbarButton(canvas, moveRect, movementPresetLabel(), Color.rgb(91, 94, 112))
            drawToolbarButton(
                canvas, gridRect, if (gridEnabled) "شبکه ۰٫۵٪" else "شبکه",
                if (gridEnabled) Color.rgb(25, 126, 105) else Color.rgb(65, 82, 103)
            )
            drawToolbarButton(canvas, zoomInRect, "+", Color.rgb(43, 105, 133))
            drawToolbarButton(canvas, zoomOutRect, "−", Color.rgb(43, 105, 133))
            drawToolbarButton(canvas, zoomResetRect, "۱:۱", Color.rgb(79, 91, 109))
            drawToolbarButton(canvas, renameRect, "نام", Color.rgb(104, 83, 132))
            drawToolbarButton(canvas, movementKeysRect, "الگوها", Color.rgb(77, 105, 121))

            val footerTop = height - dp(66f)
            paint.color = WayTouchDesign.PANEL
            val footerPanel = RectF(dp(12f), footerTop, width - dp(12f), height - dp(12f))
            canvas.drawRoundRect(footerPanel, dp(16f), dp(16f), paint)
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = dp(1f)
            paint.color = WayTouchDesign.STROKE
            canvas.drawRoundRect(footerPanel, dp(16f), dp(16f), paint)
            paint.style = Paint.Style.FILL

            val footerButtonTop = footerTop + dp(4f)
            val footerButtonBottom = height - dp(14f)
            var footerRight = width - dp(20f)
            fun footerButton(buttonWidth: Float): RectF {
                val rect = RectF(
                    footerRight - buttonWidth, footerButtonTop,
                    footerRight, footerButtonBottom
                )
                footerRight -= buttonWidth + dp(7f)
                return rect
            }
            saveRect = footerButton(dp(96f))
            cancelRect = footerButton(dp(82f))
            redoRect = footerButton(dp(82f))
            undoRect = footerButton(dp(82f))
            testRect = footerButton(dp(88f))

            drawToolbarButton(canvas, saveRect, "ذخیره", WayTouchDesign.ACCENT_DARK)
            drawToolbarButton(canvas, cancelRect, "لغو", Color.rgb(46, 66, 84))
            drawToolbarButton(
                canvas, redoRect, "جلو",
                if (redoHistory.isEmpty()) Color.rgb(48, 58, 70) else Color.rgb(65, 94, 123)
            )
            drawToolbarButton(
                canvas, undoRect, "بازگشت",
                if (undoHistory.isEmpty()) Color.rgb(48, 58, 70) else Color.rgb(65, 94, 123)
            )
            drawToolbarButton(canvas, testRect, "تست بازی", Color.rgb(37, 117, 156))

            // Contextual property sheet: binding, name, delete, precision and nudges
            // are visible only while a marker is selected.
            if (selectionVisible) {
                val panelLeft = dp(14f)
                val panelRight = width - dp(14f)
                val panelTop = footerTop - dp(78f)
                val panelBottom = footerTop - dp(6f)
                selectionPanelRect = RectF(panelLeft, panelTop, panelRight, panelBottom)
                paint.color = WayTouchDesign.PANEL_ALT
                canvas.drawRoundRect(selectionPanelRect, dp(14f), dp(14f), paint)
                paint.style = Paint.Style.STROKE
                paint.strokeWidth = dp(1f)
                paint.color = WayTouchDesign.STROKE
                canvas.drawRoundRect(selectionPanelRect, dp(14f), dp(14f), paint)
                paint.style = Paint.Style.FILL

                val controlSize = dp(48f)
                val controlGap = dp(6f)
                var controlLeft = panelLeft + dp(10f)
                fun sheetControl(buttonWidth: Float): RectF {
                    val rect = RectF(
                        controlLeft, panelTop + dp(12f),
                        controlLeft + buttonWidth, panelTop + dp(60f)
                    )
                    controlLeft += buttonWidth + controlGap
                    return rect
                }
                nudgeLeftRect = sheetControl(controlSize)
                nudgeRightRect = sheetControl(controlSize)
                nudgeUpRect = sheetControl(controlSize)
                nudgeDownRect = sheetControl(controlSize)
                nudgeStepRect = sheetControl(dp(72f))
                drawToolbarButton(canvas, nudgeLeftRect, "←", Color.rgb(49, 69, 87))
                drawToolbarButton(canvas, nudgeRightRect, "→", Color.rgb(49, 69, 87))
                drawToolbarButton(canvas, nudgeUpRect, "↑", Color.rgb(49, 69, 87))
                drawToolbarButton(canvas, nudgeDownRect, "↓", Color.rgb(49, 69, 87))
                drawToolbarButton(
                    canvas, nudgeStepRect, nudgeStepLabels[nudgeStepIndex],
                    Color.rgb(34, 113, 111)
                )

                val actionEnabled = selected in layout.buttons.indices
                selectionRemoveRect = RectF(
                    panelRight - dp(58f), panelTop + dp(12f),
                    panelRight - dp(10f), panelTop + dp(60f)
                )
                selectionRenameRect = RectF(
                    panelRight - dp(112f), panelTop + dp(12f),
                    panelRight - dp(64f), panelTop + dp(60f)
                )
                selectionBindingRect = RectF(
                    panelRight - dp(208f), panelTop + dp(12f),
                    panelRight - dp(118f), panelTop + dp(60f)
                )
                drawToolbarButton(
                    canvas, selectionBindingRect, "اتصال",
                    if (actionEnabled) Color.rgb(36, 126, 108) else Color.rgb(48, 58, 70)
                )
                drawToolbarButton(
                    canvas, selectionRenameRect, "نام",
                    if (actionEnabled) Color.rgb(87, 101, 142) else Color.rgb(48, 58, 70)
                )
                drawToolbarButton(
                    canvas, selectionRemoveRect, "حذف",
                    if (actionEnabled) Color.rgb(143, 72, 76) else Color.rgb(48, 58, 70)
                )

                val summary = when (selected) {
                    -1 -> "مرکز جوی‌استیک"
                    -2 -> "شعاع جوی‌استیک"
                    in layout.buttons.indices -> layout.buttons[selected].name
                    else -> selectedName
                }
                paint.color = WayTouchDesign.TEXT
                paint.textSize = dp(13f)
                paint.typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
                val summaryX = controlLeft + dp(12f)
                val summaryMax = selectionBindingRect.left - dp(12f)
                if (summaryMax > summaryX) {
                    val available = summaryMax - summaryX
                    val fitted = if (paint.measureText(summary) > available) {
                        android.text.TextUtils.ellipsize(
                            summary, android.text.TextPaint(paint), available,
                            android.text.TextUtils.TruncateAt.END
                        ).toString()
                    } else summary
                    canvas.drawText(fitted, summaryX, selectionPanelRect.centerY() + dp(4f), paint)
                }
            } else {
                selectionPanelRect = RectF()
                selectionBindingRect = RectF()
                selectionRenameRect = RectF()
                selectionRemoveRect = RectF()
                nudgeStepRect = RectF()
                nudgeLeftRect = RectF()
                nudgeRightRect = RectF()
                nudgeUpRect = RectF()
                nudgeDownRect = RectF()
            }

            // Markers are the foremost layer: footer/toolbars must not hide the
            // real in-game target, especially controls positioned near screen edges.
            canvas.save()
            canvas.clipRect(mapViewport)
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = dp(1.4f)
            paint.pathEffect = snapGuidePathEffect
            paint.color = WayTouchDesign.AMBER
            snapGuideX?.let { guide ->
                val gx = preview.left + guide * preview.width()
                canvas.drawLine(gx, preview.top, gx, preview.bottom, paint)
            }
            snapGuideY?.let { guide ->
                val gy = preview.top + guide * preview.height()
                canvas.drawLine(preview.left, gy, preview.right, gy, paint)
            }
            paint.pathEffect = null
            paint.style = Paint.Style.FILL

            val mx = preview.left + layout.centerX * preview.width()
            val my = preview.top + layout.centerY * preview.height()
            val moveRadius = max(dp(28f), layout.radius * min(preview.width(), preview.height()))
            paint.color = Color.argb(42, 83, 190, 224)
            canvas.drawCircle(mx, my, moveRadius, paint)
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = dp(2f)
            paint.color = if (selected == -1 || selected == -2) Color.WHITE else Color.rgb(86, 203, 230)
            canvas.drawCircle(mx, my, moveRadius, paint)
            paint.style = Paint.Style.FILL
            drawMarker(canvas, mx, my, dp(25f), "WASD", Color.rgb(48, 161, 205), selected == -1)
            drawMarker(
                canvas, mx + moveRadius, my, dp(10f), "R",
                Color.rgb(218, 171, 88), selected == -2
            )

            layout.buttons.forEachIndexed { index, item ->
                val bx = preview.left + item.x * preview.width()
                val by = preview.top + item.y * preview.height()
                drawMarker(
                    canvas, bx, by, dp(27f), triggerLabel(item.source, item.code),
                    Color.rgb(50, 175, 130), selected == index
                )
            }

            if (selected in layout.buttons.indices) {
                val item = layout.buttons[selected]
                val coordinateText = String.format(
                    java.util.Locale.US, "x %.3f  ·  y %.3f", item.x, item.y
                )
                paint.textSize = dp(10f)
                paint.typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
                val chipWidth = paint.measureText(coordinateText) + dp(18f)
                val chipHeight = dp(25f)
                val markerX = preview.left + item.x * preview.width()
                val markerY = preview.top + item.y * preview.height()
                val minChipLeft = preview.left + dp(4f)
                val maxChipLeft = (preview.right - chipWidth - dp(4f)).coerceAtLeast(minChipLeft)
                val minChipTop = preview.top + dp(4f)
                val maxChipTop = (preview.bottom - chipHeight - dp(4f)).coerceAtLeast(minChipTop)
                val chipLeft = (markerX - chipWidth / 2f).coerceIn(minChipLeft, maxChipLeft)
                val chipTop = (markerY + dp(37f)).coerceIn(minChipTop, maxChipTop)
                val chip = RectF(chipLeft, chipTop, chipLeft + chipWidth, chipTop + chipHeight)
                paint.color = WayTouchDesign.ACCENT
                canvas.drawRoundRect(chip, dp(7f), dp(7f), paint)
                paint.color = 0xFF06201E.toInt()
                canvas.drawText(
                    coordinateText, chip.left + dp(9f), chip.centerY() + dp(3.5f), paint
                )
            }
            canvas.restore()

            if (triggerPickerOpen) drawTriggerPicker(canvas)
        }

        private fun rebuildPreview() {
            if (basePreview.isEmpty) return
            val centerX = basePreview.centerX() + panX
            val centerY = basePreview.centerY() + panY
            val halfWidth = basePreview.width() * zoom / 2f
            val halfHeight = basePreview.height() * zoom / 2f
            preview = RectF(centerX - halfWidth, centerY - halfHeight,
                centerX + halfWidth, centerY + halfHeight)
            clampPan()
            val clampedCenterX = basePreview.centerX() + panX
            val clampedCenterY = basePreview.centerY() + panY
            preview.set(clampedCenterX - halfWidth, clampedCenterY - halfHeight,
                clampedCenterX + halfWidth, clampedCenterY + halfHeight)
        }

        private fun clampPan() {
            if (basePreview.isEmpty || mapViewport.isEmpty) return
            val halfOverflowX = max(0f, (basePreview.width() * zoom - mapViewport.width()) / 2f)
            val halfOverflowY = max(0f, (basePreview.height() * zoom - mapViewport.height()) / 2f)
            panX = panX.coerceIn(-halfOverflowX, halfOverflowX)
            panY = panY.coerceIn(-halfOverflowY, halfOverflowY)
        }

        private fun changeZoom(factor: Float) {
            val next = (zoom * factor).coerceIn(1f, 5f)
            if (next == zoom) return
            zoom = next
            if (zoom <= 1f) {
                panX = 0f
                panY = 0f
            }
            rebuildPreview()
            selectedName = "بزرگ‌نمایی " + String.format(java.util.Locale.US, "%.2f", zoom)
            invalidate()
        }

        private fun resetZoom() {
            zoom = 1f
            panX = 0f
            panY = 0f
            rebuildPreview()
            selectedName = "بزرگ‌نمایی ۱:۱"
            invalidate()
        }

        private fun toolbarIcon(label: String): String? = when {
            label.startsWith("+کلید") -> "⌨"
            label.startsWith("+ماوس") -> "◉"
            label == "اتصال" -> "↔"
            label == "حذف" -> "×"
            label.startsWith("شبکه") -> "▦"
            label == "۱:۱" -> "⌖"
            label == "نام" -> "✎"
            label == "الگوها" -> "▣"
            label == "ذخیره" -> "✓"
            label == "لغو" -> "×"
            label == "جلو" -> "↪"
            label == "بازگشت" -> "↶"
            label == "تست بازی" -> "▶"
            else -> null
        }

        private fun drawToolbarButton(canvas: Canvas, rect: RectF, label: String, color: Int) {
            paint.color = color
            canvas.drawRoundRect(rect, dp(9f), dp(9f), paint)
            val icon = toolbarIcon(label)
            if (icon == null || rect.height() < dp(44f)) {
                drawCentered(canvas, label, rect, Color.WHITE)
                return
            }
            paint.color = Color.WHITE
            paint.typeface = Typeface.create("sans-serif", Typeface.NORMAL)
            paint.textSize = dp(15f)
            canvas.drawText(
                icon,
                rect.centerX() - paint.measureText(icon) / 2f,
                rect.top + dp(19f),
                paint
            )
            paint.textSize = dp(9.5f)
            paint.typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            val caption = if (label == "تست بازی") "تست" else label
            val fitted = android.text.TextUtils.ellipsize(
                caption, android.text.TextPaint(paint), rect.width() - dp(4f),
                android.text.TextUtils.TruncateAt.END
            ).toString()
            canvas.drawText(
                fitted,
                rect.centerX() - paint.measureText(fitted) / 2f,
                rect.bottom - dp(5f),
                paint
            )
        }

        private fun drawTriggerPicker(canvas: Canvas) {
            canvas.drawColor(Color.argb(205, 2, 6, 12))
            val modalWidth = min(dp(720f), width - dp(36f))
            val modalHeight = min(dp(430f), height - dp(180f))
            val modal = RectF(
                (width - modalWidth) / 2f,
                (height - modalHeight) / 2f,
                (width + modalWidth) / 2f,
                (height + modalHeight) / 2f
            )
            paint.color = Color.rgb(17, 29, 43)
            canvas.drawRoundRect(modal, dp(20f), dp(20f), paint)
            paint.color = Color.rgb(76, 105, 130)
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = dp(1f)
            canvas.drawRoundRect(modal, dp(20f), dp(20f), paint)
            paint.style = Paint.Style.FILL
            paint.color = Color.WHITE
            paint.textSize = dp(18f)
            paint.typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            canvas.drawText("انتخاب کلید یا دکمهٔ ماوس", modal.left + dp(22f), modal.top + dp(34f), paint)

            pickerCloseRect = RectF(modal.right - dp(108f), modal.top + dp(12f),
                modal.right - dp(16f), modal.top + dp(48f))
            pickerSearchRect = RectF(modal.left + dp(18f), modal.top + dp(12f),
                modal.left + dp(128f), modal.top + dp(48f))
            drawToolbarButton(canvas, pickerCloseRect, "بستن", Color.rgb(73, 83, 98))
            drawToolbarButton(canvas, pickerSearchRect, "جست‌وجو", Color.rgb(44, 104, 118))

            val columns = 4
            val gap = dp(10f)
            val innerLeft = modal.left + dp(20f)
            val innerTop = modal.top + dp(70f)
            val cellWidth = (modal.width() - dp(40f) - gap * (columns - 1)) / columns
            val cellHeight = dp(48f)
            val pageCount = ((pickerOptions.size + pickerPageSize - 1) / pickerPageSize).coerceAtLeast(1)
            pickerPage = pickerPage.coerceIn(0, pageCount - 1)
            val pageOptions = pickerOptions.drop(pickerPage * pickerPageSize).take(pickerPageSize)
            pickerRects.clear()
            pageOptions.forEachIndexed { index, option ->
                val column = index % columns
                val row = index / columns
                val left = innerLeft + column * (cellWidth + gap)
                val top = innerTop + row * (cellHeight + gap)
                val rect = RectF(left, top, left + cellWidth, top + cellHeight)
                pickerRects += rect
                val taken = layout.buttons.withIndex().any {
                    it.index != selected && it.value.source == option.source && it.value.code == option.code
                }
                val chosen = selected in layout.buttons.indices
                    && layout.buttons[selected].source == option.source
                    && layout.buttons[selected].code == option.code
                paint.color = when {
                    taken -> Color.rgb(48, 54, 63)
                    chosen -> Color.rgb(30, 112, 112)
                    option.source == "key" -> Color.rgb(38, 77, 108)
                    else -> Color.rgb(37, 104, 87)
                }
                canvas.drawRoundRect(rect, dp(10f), dp(10f), paint)
                drawCentered(
                    canvas,
                    option.label + if (taken) " · استفاده‌شده" else "",
                    rect,
                    if (taken) Color.rgb(125, 134, 145) else Color.WHITE
                )
            }
            val navTop = modal.bottom - dp(48f)
            pickerPreviousRect = RectF(modal.left + dp(20f), navTop, modal.left + dp(110f), modal.bottom - dp(10f))
            pickerNextRect = RectF(modal.right - dp(110f), navTop, modal.right - dp(20f), modal.bottom - dp(10f))
            drawToolbarButton(
                canvas, pickerPreviousRect, "‹",
                if (pickerPage > 0) Color.rgb(49, 88, 122) else Color.rgb(48, 54, 63)
            )
            drawToolbarButton(
                canvas, pickerNextRect, "›",
                if (pickerPage < pageCount - 1) Color.rgb(49, 88, 122) else Color.rgb(48, 54, 63)
            )
            paint.color = Color.rgb(190, 208, 222)
            paint.textSize = dp(12f)
            val pageLabel = "صفحه " + (pickerPage + 1) + "/" + pageCount + " · " + pickerOptions.size + " ورودی"
            canvas.drawText(pageLabel, modal.centerX() - paint.measureText(pageLabel) / 2f, navTop + dp(25f), paint)
        }

        private fun triggerLabel(source: String, code: Int): String {
            val options = if (source == "key") keyOptions else mouseOptions
            return options.firstOrNull { it.code == code }?.label ?: code.toString()
        }

        private fun reservedKey(code: Int): Boolean =
            code in setOf(1, 29, 56, 59, 60, 61, 97, 100) ||
                code == layout.upKey || code == layout.leftKey ||
                code == layout.downKey || code == layout.rightKey

        private fun triggerTaken(option: TriggerOption, exceptIndex: Int = Int.MIN_VALUE): Boolean {
            if (option.source == "key" && reservedKey(option.code)) return true
            return layout.buttons.withIndex().any {
                it.index != exceptIndex && it.value.source == option.source && it.value.code == option.code
            }
        }

        private fun movementPresetIndex(): Int = when {
            layout.upKey == 17 && layout.leftKey == 30 && layout.downKey == 31 && layout.rightKey == 32 -> 0
            layout.upKey == 103 && layout.leftKey == 105 && layout.downKey == 108 && layout.rightKey == 106 -> 1
            layout.upKey == 72 && layout.leftKey == 75 && layout.downKey == 80 && layout.rightKey == 77 -> 2
            else -> -1
        }

        private fun movementPresetLabel(): String = when (movementPresetIndex()) {
            0 -> "WASD"
            1 -> "جهت‌ها"
            2 -> "صفحه‌کلید عددی"
            else -> "سفارشی"
        }

        private fun showPresetPicker() {
            val choices = arrayOf("الگوی تیراندازی (FPS)", "الگوی MOBA", "تنظیم دستی کلیدهای حرکت")
            AlertDialog.Builder(this@EditorService)
                .setTitle("الگوهای آماده")
                .setItems(choices) { _, which ->
                    when (which) {
                        0 -> confirmApplyGamePreset("FPS")
                        1 -> confirmApplyGamePreset("MOBA")
                        2 -> editMovementKeys()
                    }
                }
                .setNegativeButton("بستن", null)
                .create()
                .also {
                    it.window?.setType(WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY)
                    it.show()
                }
        }

        private fun confirmApplyGamePreset(preset: String) {
            val title = if (preset == "FPS") "الگوی تیراندازی (FPS)" else "الگوی MOBA"
            AlertDialog.Builder(this@EditorService)
                .setTitle(title)
                .setMessage("چیدمان فعلی دکمه‌ها با الگوی انتخاب‌شده جایگزین می‌شود. این تغییر با بازگشت قابل لغو است.")
                .setNegativeButton("انصراف", null)
                .setPositiveButton("اعمال الگو") { _, _ -> applyGamePreset(preset) }
                .create()
                .also {
                    it.window?.setType(WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY)
                    it.show()
                }
        }

        private fun applyGamePreset(preset: String) {
            pushUndo()
            layout.centerX = 0.16f
            layout.centerY = 0.78f
            layout.radius = 0.07f
            layout.upKey = 17
            layout.leftKey = 30
            layout.downKey = 31
            layout.rightKey = 32
            layout.buttons.clear()

            val controls = if (preset == "FPS") {
                listOf(
                    EditorButton("jump", 0.86f, 0.80f, "key", 57),
                    EditorButton("reload", 0.84f, 0.58f, "key", 19),
                    EditorButton("fire", 0.94f, 0.42f, "mouse", 272),
                    EditorButton("aim", 0.80f, 0.42f, "mouse", 273),
                    EditorButton("crouch", 0.73f, 0.80f, "key", 46),
                    EditorButton("sprint", 0.16f, 0.63f, "key", 42),
                    EditorButton("interact", 0.74f, 0.62f, "key", 33)
                )
            } else {
                listOf(
                    EditorButton("attack", 0.91f, 0.80f, "mouse", 272),
                    EditorButton("skill_q", 0.78f, 0.68f, "key", 16),
                    EditorButton("skill_e", 0.86f, 0.55f, "key", 18),
                    EditorButton("skill_r", 0.94f, 0.62f, "key", 19),
                    EditorButton("skill_d", 0.72f, 0.45f, "key", 33),
                    EditorButton("skill_f", 0.82f, 0.42f, "key", 34),
                    EditorButton("recall", 0.91f, 0.42f, "key", 48),
                    EditorButton("item_z", 0.68f, 0.80f, "key", 44),
                    EditorButton("item_x", 0.76f, 0.88f, "key", 45)
                )
            }
            layout.buttons.addAll(controls)
            selected = Int.MIN_VALUE
            selectedName = if (preset == "FPS") "الگوی FPS" else "الگوی MOBA"
            invalidate()
            Toast.makeText(
                applicationContext,
                "الگوی $preset اعمال شد؛ برای ثبت نهایی ذخیره کنید.",
                Toast.LENGTH_LONG
            ).show()
        }

        private fun editMovementKeys() {
            val reservedSystemKeys = setOf(1, 29, 56, 59, 60, 97, 100)
            val currentCodes = listOf(layout.upKey, layout.leftKey, layout.downKey, layout.rightKey)
            val actionCodes = layout.buttons.filter { it.source == "key" }.map { it.code }.toSet()
            val options = (
                keyOptions.filter { it.code !in reservedSystemKeys && it.code !in actionCodes } +
                    currentCodes.map { code ->
                        keyOptions.firstOrNull { it.code == code } ?: TriggerOption("KEY_$code", "key", code)
                    }
                ).distinctBy { it.code }
            val adapter = ArrayAdapter(
                this@EditorService,
                android.R.layout.simple_spinner_item,
                options.map { it.label }
            ).apply { setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item) }
            val container = LinearLayout(this@EditorService).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(dp(20f).toInt(), dp(8f).toInt(), dp(20f).toInt(), dp(8f).toInt())
            }
            val labels = listOf("بالا", "چپ", "پایین", "راست")
            val spinners = currentCodes.mapIndexed { index, code ->
                val row = LinearLayout(this@EditorService).apply {
                    orientation = LinearLayout.VERTICAL
                    setPadding(0, dp(4f).toInt(), 0, dp(4f).toInt())
                }
                row.addView(TextView(this@EditorService).apply {
                    text = labels[index]
                    textSize = 13f
                })
                val spinner = Spinner(this@EditorService).apply {
                    this.adapter = adapter
                    setSelection(options.indexOfFirst { it.code == code }.coerceAtLeast(0))
                }
                row.addView(spinner)
                container.addView(row)
                spinner
            }
            AlertDialog.Builder(this@EditorService)
                .setTitle("کلیدهای دلخواه حرکت")
                .setView(container)
                .setNegativeButton("انصراف", null)
                .setPositiveButton("اعمال") { _, _ ->
                    val codes = spinners.map { options[it.selectedItemPosition].code }
                    val duplicate = codes.toSet().size != 4
                    val conflict = layout.buttons.any { it.source == "key" && it.code in codes }
                    if (duplicate || conflict) {
                        Toast.makeText(
                            applicationContext,
                            "چهار کلید باید متفاوت باشند و با دکمه‌های عملیاتی تداخل نداشته باشند.",
                            Toast.LENGTH_LONG
                        ).show()
                    } else {
                        pushUndo()
                        layout.upKey = codes[0]
                        layout.leftKey = codes[1]
                        layout.downKey = codes[2]
                        layout.rightKey = codes[3]
                        selectedName = "کلیدهای حرکت سفارشی"
                        invalidate()
                    }
                }
                .create()
                .also {
                    it.window?.setType(WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY)
                    it.show()
                }
        }

        private fun cycleMovementPreset() {
            val presets = listOf(
                intArrayOf(17, 30, 31, 32),
                intArrayOf(103, 105, 108, 106),
                intArrayOf(72, 75, 80, 77)
            )
            val next = (movementPresetIndex() + 1) % presets.size
            val codes = presets[next]
            if (layout.buttons.any { it.source == "key" && it.code in codes }) {
                Toast.makeText(
                    applicationContext,
                    "این کلید در یکی از دکمه‌ها استفاده شده است؛ ابتدا Binding آن را تغییر دهید.",
                    Toast.LENGTH_LONG
                ).show()
                return
            }
            pushUndo()
            layout.upKey = codes[0]
            layout.leftKey = codes[1]
            layout.downKey = codes[2]
            layout.rightKey = codes[3]
            selectedName = movementPresetLabel()
            invalidate()
        }

        private fun addControl(source: String) {
            val options = if (source == "key") keyOptions else mouseOptions
            val option = options.firstOrNull { !triggerTaken(it) }
            if (option == null) {
                Toast.makeText(applicationContext, "ورودی آزاد دیگری برای این نوع وجود ندارد.", Toast.LENGTH_LONG).show()
                return
            }
            var suffix = layout.buttons.size + 1
            while (layout.buttons.any { it.name == "action_$suffix" }) suffix++
            pushUndo()
            val index = layout.buttons.size
            val x = (0.66f + (index % 3) * 0.13f).coerceIn(0.08f, 0.94f)
            val y = (0.55f + (index / 3) * 0.14f).coerceIn(0.08f, 0.94f)
            layout.buttons += EditorButton("action_$suffix", x, y, option.source, option.code)
            selected = layout.buttons.lastIndex
            selectedName = layout.buttons[selected].name
            invalidate()
            Toast.makeText(
                applicationContext,
                "کنترل ${option.label} اضافه شد؛ برای تغییر ورودی آن Edit را بزنید.",
                Toast.LENGTH_SHORT
            ).show()
        }

        private fun removeSelectedControl() {
            if (selected !in layout.buttons.indices) {
                Toast.makeText(applicationContext, "ابتدا یک دکمه را انتخاب کنید.", Toast.LENGTH_SHORT).show()
                return
            }
            pushUndo()
            val removed = layout.buttons.removeAt(selected)
            selected = Int.MIN_VALUE
            selectedName = "—"
            invalidate()
            Toast.makeText(applicationContext, "${removed.name} حذف شد.", Toast.LENGTH_SHORT).show()
        }

        private fun openTriggerPicker() {
            if (selected !in layout.buttons.indices) {
                Toast.makeText(applicationContext, "ابتدا دکمه‌ای را روی صفحه انتخاب کنید.", Toast.LENGTH_SHORT).show()
                return
            }
            pickerAllOptions = (keyOptions + mouseOptions).filter { !triggerTaken(it, selected) }
            pickerOptions = pickerAllOptions
            pickerPage = 0
            triggerPickerOpen = true
            invalidate()
        }

        private fun searchTriggerOptions() {
            val query = EditText(this@EditorService).apply {
                setSingleLine(true)
                hint = "مثلاً F5 یا SPACE"
                setPadding(dp(16f).toInt(), dp(8f).toInt(), dp(16f).toInt(), dp(8f).toInt())
            }
            AlertDialog.Builder(this@EditorService)
                .setTitle("جست‌وجوی کلید یا دکمه")
                .setView(query)
                .setNegativeButton("بستن", null)
                .setNeutralButton("پاک کردن") { _, _ ->
                    pickerOptions = pickerAllOptions
                    pickerPage = 0
                    invalidate()
                }
                .setPositiveButton("جست‌وجو") { _, _ ->
                    val term = query.text.toString().trim()
                    pickerOptions = if (term.isEmpty()) {
                        pickerAllOptions
                    } else {
                        pickerAllOptions.filter {
                            it.label.contains(term, ignoreCase = true) ||
                                it.code.toString() == term
                        }
                    }
                    pickerPage = 0
                    invalidate()
                }
                .create()
                .also {
                    it.window?.setType(WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY)
                    it.show()
                }
        }

        private fun chooseTrigger(index: Int) {
            if (selected !in layout.buttons.indices || index !in pickerOptions.indices) return
            val option = pickerOptions[index]
            if (triggerTaken(option, selected)) {
                Toast.makeText(applicationContext, "این ورودی قبلاً استفاده شده است.", Toast.LENGTH_SHORT).show()
                return
            }
            val button = layout.buttons[selected]
            pushUndo()
            button.source = option.source
            button.code = option.code
            selectedName = button.name
            triggerPickerOpen = false
            invalidate()
            Toast.makeText(
                applicationContext,
                "ورودی ${button.name} روی ${option.label} تنظیم شد.",
                Toast.LENGTH_SHORT
            ).show()
        }

        private fun drawMarker(canvas: Canvas, x: Float, y: Float, radius: Float,
                               label: String, color: Int, active: Boolean) {
            if (active) {
                paint.style = Paint.Style.STROKE
                paint.strokeWidth = dp(1.8f)
                paint.pathEffect = selectionRingPathEffect
                paint.color = Color.WHITE
                canvas.drawCircle(x, y, radius + dp(8f), paint)
                paint.pathEffect = null
                paint.style = Paint.Style.FILL
            }
            paint.color = color
            canvas.drawCircle(x, y, radius, paint)
            paint.color = Color.WHITE
            paint.textSize = dp(9f)
            paint.typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            canvas.drawText(label, x - paint.measureText(label) / 2f, y + dp(3f), paint)
        }

        private fun drawCentered(canvas: Canvas, text: String, rect: RectF, color: Int) {
            paint.color = color
            paint.textSize = dp(13f)
            paint.typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            canvas.drawText(text, rect.centerX() - paint.measureText(text) / 2f,
                rect.centerY() + dp(5f), paint)
        }

        override fun onTouchEvent(event: MotionEvent): Boolean {
            scaleDetector.onTouchEvent(event)
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> {
                    twoFingerPanning = false
                    Log.i(TAG, "Touch down x=" + event.x + " y=" + event.y + " view=" + width + "x" + height)
                    if (triggerPickerOpen) {
                        if (pickerSearchRect.contains(event.x, event.y)) {
                            searchTriggerOptions()
                            return true
                        }
                        if (pickerPreviousRect.contains(event.x, event.y)) {
                            pickerPage = (pickerPage - 1).coerceAtLeast(0)
                            invalidate()
                            return true
                        }
                        if (pickerNextRect.contains(event.x, event.y)) {
                            val pages = (pickerOptions.size + pickerPageSize - 1) / pickerPageSize
                            pickerPage = (pickerPage + 1).coerceAtMost((pages - 1).coerceAtLeast(0))
                            invalidate()
                            return true
                        }
                        if (pickerCloseRect.contains(event.x, event.y)) {
                            triggerPickerOpen = false
                            invalidate()
                            return true
                        }
                        val optionIndex = pickerRects.indexOfFirst { it.contains(event.x, event.y) }
                        if (optionIndex >= 0) chooseTrigger(pickerPage * pickerPageSize + optionIndex)
                        return true
                    }
                    val markerHit = findNearest(event.x, event.y, dp(34f))
                    if (markerHit != Int.MIN_VALUE) {
                        selected = markerHit
                        performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
                        dragHapticSent = false
                        wasSnapped = false
                        snapGuideX = null
                        snapGuideY = null
                        selectedName = when (selected) {
                            -1 -> "مرکز جوی‌استیک"
                            -2 -> "شعاع جوی‌استیک"
                            in layout.buttons.indices -> (
                                layout.buttons[selected].name + " · " +
                                    triggerLabel(layout.buttons[selected].source, layout.buttons[selected].code)
                                )
                            else -> "—"
                        }
                        panning = false
                        lastPanX = event.x
                        lastPanY = event.y
                        dragging = true
                        pushUndo()
                        invalidate()
                        return true
                    }
                    if (addKeyRect.contains(event.x, event.y)) {
                        addControl("key")
                        return true
                    }
                    if (addMouseRect.contains(event.x, event.y)) {
                        addControl("mouse")
                        return true
                    }
                    if (editRect.contains(event.x, event.y)) {
                        openTriggerPicker()
                        return true
                    }
                    if (removeRect.contains(event.x, event.y)) {
                        removeSelectedControl()
                        return true
                    }
                    if (renameRect.contains(event.x, event.y)) {
                        renameSelectedControl()
                        return true
                    }
                    if (undoRect.contains(event.x, event.y)) {
                        undoLastChange()
                        return true
                    }
                    if (redoRect.contains(event.x, event.y)) {
                        redoLastChange()
                        return true
                    }
                    if (testRect.contains(event.x, event.y)) {
                        testEditorInGame(this)
                        return true
                    }
                    if (selectionBindingRect.contains(event.x, event.y) && selected in layout.buttons.indices) {
                        openTriggerPicker()
                        return true
                    }
                    if (selectionRenameRect.contains(event.x, event.y) && selected in layout.buttons.indices) {
                        renameSelectedControl()
                        return true
                    }
                    if (selectionRemoveRect.contains(event.x, event.y) && selected in layout.buttons.indices) {
                        removeSelectedControl()
                        return true
                    }
                    if (nudgeStepRect.contains(event.x, event.y)) {
                        nudgeStepIndex = (nudgeStepIndex + 1) % nudgeSteps.size
                        performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
                        invalidate()
                        return true
                    }
                    if (moveRect.contains(event.x, event.y)) {
                        cycleMovementPreset()
                        return true
                    }
                    if (movementKeysRect.contains(event.x, event.y)) {
                        showPresetPicker()
                        return true
                    }
                    if (zoomInRect.contains(event.x, event.y)) {
                        changeZoom(1.25f)
                        return true
                    }
                    if (zoomOutRect.contains(event.x, event.y)) {
                        changeZoom(0.8f)
                        return true
                    }
                    if (zoomResetRect.contains(event.x, event.y)) {
                        resetZoom()
                        return true
                    }
                    if (gridRect.contains(event.x, event.y)) {
                        gridEnabled = !gridEnabled
                        Toast.makeText(
                            applicationContext,
                            if (gridEnabled) "شبکهٔ ۰٫۵٪ فعال شد." else "شبکهٔ دقت غیرفعال شد.",
                            Toast.LENGTH_SHORT
                        ).show()
                        invalidate()
                        return true
                    }
                    if (nudgeLeftRect.contains(event.x, event.y)) { nudgeSelected(-1, 0); return true }
                    if (nudgeRightRect.contains(event.x, event.y)) { nudgeSelected(1, 0); return true }
                    if (nudgeUpRect.contains(event.x, event.y)) { nudgeSelected(0, -1); return true }
                    if (nudgeDownRect.contains(event.x, event.y)) { nudgeSelected(0, 1); return true }
                    if (saveRect.contains(event.x, event.y)) {
                        Log.i(TAG, "Save control tapped")
                        finishEditor(true, layout)
                        return true
                    }
                    if (cancelRect.contains(event.x, event.y)) {
                        confirmCancel()
                        return true
                    }
                    selected = findNearest(event.x, event.y)
                    if (selected != Int.MIN_VALUE) {
                        performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
                    }
                    dragHapticSent = false
                    wasSnapped = false
                    snapGuideX = null
                    snapGuideY = null
                    Log.i(TAG, "Selected marker=" + selected)
                    selectedName = when (selected) {
                        -1 -> "مرکز جوی‌استیک"
                        -2 -> "شعاع جوی‌استیک"
                        in layout.buttons.indices -> (
                            layout.buttons[selected].name + " · " +
                                triggerLabel(layout.buttons[selected].source, layout.buttons[selected].code)
                            )
                        else -> "—"
                    }
                    panning = selected == Int.MIN_VALUE && zoom > 1f && mapViewport.contains(event.x, event.y)
                    lastPanX = event.x
                    lastPanY = event.y
                    dragging = selected != Int.MIN_VALUE
                    if (dragging) pushUndo()
                    if (panning) selectedName = "جابجایی نقشه · بزرگ‌نمایی " + String.format(java.util.Locale.US, "%.2f", zoom)
                    // Do not jump the selected marker to the finger-down point.
                    // Position only changes after an actual drag or a nudge.
                    invalidate()
                    return true
                }
                MotionEvent.ACTION_POINTER_DOWN -> {
                    if (!triggerPickerOpen && event.pointerCount >= 2) {
                        twoFingerPanning = true
                        panning = false
                        dragging = false
                        twoFingerFocusX = (event.getX(0) + event.getX(1)) / 2f
                        twoFingerFocusY = (event.getY(0) + event.getY(1)) / 2f
                        return true
                    }
                }
                MotionEvent.ACTION_MOVE -> {
                    if (twoFingerPanning && event.pointerCount >= 2) {
                        val focusX = (event.getX(0) + event.getX(1)) / 2f
                        val focusY = (event.getY(0) + event.getY(1)) / 2f
                        if (zoom > 1f && mapViewport.contains(focusX, focusY)) {
                            panX += focusX - twoFingerFocusX
                            panY += focusY - twoFingerFocusY
                            rebuildPreview()
                        }
                        twoFingerFocusX = focusX
                        twoFingerFocusY = focusY
                        invalidate()
                        return true
                    }
                    if (panning) {
                        panX += event.x - lastPanX
                        panY += event.y - lastPanY
                        lastPanX = event.x
                        lastPanY = event.y
                        rebuildPreview()
                        invalidate()
                    } else if (dragging) {
                        if (!dragHapticSent) {
                            performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
                            dragHapticSent = true
                        }
                        updatePosition(event.x, event.y)
                        invalidate()
                    }
                    return true
                }
                MotionEvent.ACTION_POINTER_UP -> {
                    if (event.pointerCount <= 2) twoFingerPanning = false
                    return true
                }
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                    Log.i(TAG, "Touch end action=" + event.actionMasked + " marker=" + selected)
                    dragging = false
                    panning = false
                    twoFingerPanning = false
                    dragHapticSent = false
                    wasSnapped = false
                    snapGuideX = null
                    snapGuideY = null
                    invalidate()
                    return true
                }
            }
            return true
        }

        private fun findNearest(x: Float, y: Float, hitRadius: Float = dp(68f)): Int {
            var best = Int.MIN_VALUE
            var bestDistance = hitRadius
            val points = mutableListOf<Pair<Int, Pair<Float, Float>>>()
            val centerX = preview.left + layout.centerX * preview.width()
            val centerY = preview.top + layout.centerY * preview.height()
            points += Pair(-1, Pair(centerX, centerY))
            val displayedRadius = max(dp(28f), layout.radius * min(preview.width(), preview.height()))
            points += Pair(-2, Pair(centerX + displayedRadius, centerY))
            layout.buttons.forEachIndexed { index, item ->
                points += Pair(index, Pair(
                    preview.left + item.x * preview.width(),
                    preview.top + item.y * preview.height()
                ))
            }
            for ((id, point) in points) {
                val dx = x - point.first
                val dy = y - point.second
                val distance = sqrt(dx * dx + dy * dy)
                if (distance < bestDistance) {
                    best = id
                    bestDistance = distance
                }
            }
            return best
        }

        private fun updatePosition(x: Float, y: Float) {
            if (!dragging || preview.width() <= 0f || preview.height() <= 0f) return
            var nx = ((x - preview.left) / preview.width()).coerceIn(0f, 1f)
            var ny = ((y - preview.top) / preview.height()).coerceIn(0f, 1f)
            if (gridEnabled) {
                nx = (round(nx / 0.005f) * 0.005f).coerceIn(0f, 1f)
                ny = (round(ny / 0.005f) * 0.005f).coerceIn(0f, 1f)
            }

            snapGuideX = null
            snapGuideY = null
            if (selected == -1 || selected in layout.buttons.indices) {
                val xAnchors = mutableListOf<Float>()
                val yAnchors = mutableListOf<Float>()
                if (selected != -1) {
                    xAnchors += layout.centerX
                    yAnchors += layout.centerY
                }
                layout.buttons.forEachIndexed { index, button ->
                    if (index != selected) {
                        xAnchors += button.x
                        yAnchors += button.y
                    }
                }
                snapGuideX = xAnchors.minByOrNull { abs(it - nx) }
                    ?.takeIf { abs(it - nx) <= 0.008f }
                snapGuideY = yAnchors.minByOrNull { abs(it - ny) }
                    ?.takeIf { abs(it - ny) <= 0.008f }
                snapGuideX?.let { nx = it }
                snapGuideY?.let { ny = it }
                val nowSnapped = snapGuideX != null || snapGuideY != null
                if (nowSnapped && !wasSnapped) {
                    performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
                }
                wasSnapped = nowSnapped
            } else {
                wasSnapped = false
            }

            when {
                selected == -1 -> {
                    layout.centerX = nx
                    layout.centerY = ny
                    selectedName = "مرکز · x=" + String.format(java.util.Locale.US, "%.3f", nx) + " y=" + String.format(java.util.Locale.US, "%.3f", ny)
                }
                selected == -2 -> {
                    val centerX = preview.left + layout.centerX * preview.width()
                    val centerY = preview.top + layout.centerY * preview.height()
                    val dx = x - centerX
                    val dy = y - centerY
                    val minDimension = min(preview.width(), preview.height()).coerceAtLeast(1f)
                    layout.radius = (sqrt(dx * dx + dy * dy) / minDimension).coerceIn(0.01f, 0.5f)
                    selectedName = "شعاع · " + String.format(java.util.Locale.US, "%.3f", layout.radius)
                }
                selected in layout.buttons.indices -> {
                    layout.buttons[selected].x = nx
                    layout.buttons[selected].y = ny
                    val button = layout.buttons[selected]
                    selectedName = button.name + " · x=" + String.format(java.util.Locale.US, "%.3f", nx) + " y=" + String.format(java.util.Locale.US, "%.3f", ny)
                }
            }
        }

        private fun nudgeSelected(dx: Int, dy: Int) {
            val step = nudgeSteps[nudgeStepIndex]
            if (selected !in layout.buttons.indices && selected != -1 && selected != -2) {
                Toast.makeText(applicationContext, "ابتدا یک کنترل را انتخاب کنید.", Toast.LENGTH_SHORT).show()
                return
            }
            pushUndo()
            when {
                selected == -1 -> {
                    layout.centerX = (layout.centerX + dx * step).coerceIn(0f, 1f)
                    layout.centerY = (layout.centerY + dy * step).coerceIn(0f, 1f)
                    selectedName = "مرکز · x=" + String.format(java.util.Locale.US, "%.3f", layout.centerX) + " y=" + String.format(java.util.Locale.US, "%.3f", layout.centerY)
                }
                selected == -2 -> {
                    val delta = when {
                        dx < 0 || dy < 0 -> -step
                        dx > 0 || dy > 0 -> step
                        else -> 0f
                    }
                    layout.radius = (layout.radius + delta).coerceIn(0.01f, 0.5f)
                    selectedName = "شعاع · " + String.format(java.util.Locale.US, "%.3f", layout.radius)
                }
                selected in layout.buttons.indices -> {
                    val button = layout.buttons[selected]
                    button.x = (button.x + dx * step).coerceIn(0f, 1f)
                    button.y = (button.y + dy * step).coerceIn(0f, 1f)
                    selectedName = button.name + " · x=" + String.format(java.util.Locale.US, "%.3f", button.x) + " y=" + String.format(java.util.Locale.US, "%.3f", button.y)
                }
                else -> Toast.makeText(applicationContext, "ابتدا یک کنترل را انتخاب کنید.", Toast.LENGTH_SHORT).show()
            }
            invalidate()
        }
    }

    companion object {
        const val ACTION_START = "com.waytouch.editor.START"
        const val ACTION_OPEN = "com.waytouch.editor.OPEN"
        const val ACTION_CLOSE = "com.waytouch.editor.CLOSE"
        const val ACTION_STOP = "com.waytouch.editor.STOP"
        const val EXTRA_IPC_TOKEN = "com.waytouch.editor.IPC_TOKEN"
        private const val IPC_PREFS = "waytouch_ipc"
        private const val IPC_TOKEN_KEY = "session_token"
        private const val CHANNEL_ID = "waytouch_editor_controls"
        private const val NOTIFICATION_ID = 27184
        private const val CONFIG_PORT = 27184
        private const val CURSOR_PORT = 27185
        private const val TAG = "WayTouchEditor"
    }
}
