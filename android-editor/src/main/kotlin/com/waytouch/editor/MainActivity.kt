package com.waytouch.editor

import android.app.Activity
import android.content.Intent
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.View
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.net.InetSocketAddress
import java.net.Socket

class MainActivity : Activity() {
    private lateinit var permissionButton: Button
    private lateinit var status: TextView
    private lateinit var connectionStatus: TextView
    private lateinit var version: TextView
    private val mainHandler = Handler(Looper.getMainLooper())
    @Volatile private var statusGeneration = 0

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        @Suppress("DEPRECATION")
        run {
            window.statusBarColor = Color.rgb(7, 13, 22)
            window.navigationBarColor = Color.rgb(7, 13, 22)
        }

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(24), dp(28), dp(24), dp(24))
            setBackgroundColor(Color.rgb(9, 16, 26))
        }
        setContentView(ScrollView(this).apply { addView(root) })

        root.addView(TextView(this).apply {
            text = getString(R.string.app_name)
            textSize = 30f
            setTextColor(Color.rgb(235, 244, 251))
            typeface = android.graphics.Typeface.create("sans-serif-medium", 0)
            contentDescription = getString(R.string.app_name)
        })
        root.addView(TextView(this).apply {
            text = getString(R.string.main_subtitle)
            textSize = 14f
            setTextColor(Color.rgb(126, 164, 188))
            setPadding(0, dp(3), 0, dp(24))
        })

        status = TextView(this).apply {
            textSize = 14f
            setTextColor(Color.rgb(192, 211, 226))
            setPadding(dp(14), dp(14), dp(14), dp(14))
            setBackgroundColor(Color.rgb(20, 34, 48))
            accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
        }
        root.addView(status, matchWrap())

        connectionStatus = TextView(this).apply {
            textSize = 13f
            setTextColor(Color.rgb(130, 222, 196))
            setPadding(dp(14), dp(14), dp(14), dp(14))
            setBackgroundColor(Color.rgb(15, 28, 39))
            accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
            text = getString(R.string.status_checking)
        }
        root.addView(connectionStatus, matchWrap())

        version = TextView(this).apply {
            textSize = 12f
            setTextColor(Color.rgb(113, 139, 160))
            setPadding(0, dp(4), 0, dp(10))
        }
        root.addView(version, matchWrap())

        root.addView(TextView(this).apply {
            text = getString(R.string.main_description)
            textSize = 14f
            setTextColor(Color.rgb(151, 172, 190))
            setPadding(0, dp(12), 0, dp(18))
        })

        permissionButton = Button(this).apply {
            text = getString(R.string.overlay_permission)
            contentDescription = text
            setOnClickListener { requestOverlayPermission() }
        }
        root.addView(permissionButton, matchWrap())

        root.addView(Button(this).apply {
            text = getString(R.string.enable_controls)
            contentDescription = text
            setOnClickListener { startEditorService() }
        }, matchWrap())

        root.addView(Button(this).apply {
            text = getString(R.string.refresh_status)
            contentDescription = text
            setOnClickListener { refreshConnectionStatus() }
        }, matchWrap())

        root.addView(TextView(this).apply {
            text = getString(R.string.notification_help)
            textSize = 12f
            setTextColor(Color.rgb(113, 139, 160))
            setPadding(0, dp(16), 0, 0)
        })
    }

    override fun onResume() {
        super.onResume()
        updatePermissionStatus()
        refreshConnectionStatus()
    }

    private fun updatePermissionStatus() {
        val overlay = Settings.canDrawOverlays(this)
        val notifications = getSystemService(android.app.NotificationManager::class.java)
            .areNotificationsEnabled()
        status.text = buildString {
            append(if (overlay) "✓ " else "• ")
            append(getString(if (overlay) R.string.overlay_granted else R.string.overlay_required))
            append("\n")
            append(if (notifications) "✓ " else "• ")
            append(getString(if (notifications) R.string.notifications_enabled else R.string.notifications_disabled))
        }
        permissionButton.visibility = if (overlay) View.GONE else View.VISIBLE
        val versionName = try {
            @Suppress("DEPRECATION")
            packageManager.getPackageInfo(packageName, 0).versionName ?: "2.0.0"
        } catch (_: Exception) {
            "2.0.0"
        }
        version.text = getString(R.string.status_version, versionName, Build.VERSION.SDK_INT)
    }

    private fun refreshConnectionStatus() {
        val generation = ++statusGeneration
        connectionStatus.text = getString(R.string.status_checking)
        Thread({
            val token = getSharedPreferences("waytouch_ipc", MODE_PRIVATE)
                .getString("session_token", null)
            val layoutReady = token?.let { probeChannel(27184, it, "GET", "{") } ?: false
            val cursorReady = token?.let { probeChannel(27185, it, "PING", "PONG") } ?: false
            val result = if (token == null) {
                getString(R.string.session_missing)
            } else {
                getString(
                    if (layoutReady) R.string.layout_connected else R.string.layout_unavailable
                ) + "\n" + getString(
                    if (cursorReady) R.string.cursor_connected else R.string.cursor_unavailable
                )
            }
            mainHandler.post {
                if (!isFinishing && generation == statusGeneration) {
                    connectionStatus.text = result
                }
            }
        }, "waytouch-main-status").apply {
            isDaemon = true
            start()
        }
    }

    private fun probeChannel(port: Int, token: String, command: String, expected: String): Boolean {
        return try {
            Socket().use { socket ->
                socket.connect(InetSocketAddress("127.0.0.1", port), 450)
                socket.soTimeout = 650
                val input = socket.getInputStream()
                val output = socket.getOutputStream()
                output.write(("AUTH " + token + "\n").toByteArray(Charsets.US_ASCII))
                output.flush()
                if (readBoundedLine(input, 80) != "OK AUTH") return false
                output.write((command + "\n").toByteArray(Charsets.US_ASCII))
                output.flush()
                val reply = readBoundedLine(input, 16 * 1024) ?: return false
                if (expected == "{") reply.startsWith("{") else reply == expected
            }
        } catch (_: Exception) {
            false
        }
    }

    private fun readBoundedLine(input: InputStream, maxBytes: Int): String? {
        val bytes = ByteArrayOutputStream(minOf(maxBytes, 256))
        var consumed = 0
        while (true) {
            val value = input.read()
            if (value < 0) return if (consumed == 0) null else throw java.io.EOFException()
            if (value == 10) return bytes.toString(Charsets.UTF_8.name())
            consumed += 1
            if (consumed > maxBytes) throw IllegalArgumentException("status reply too large")
            if (value != 13) bytes.write(value)
        }
    }

    private fun requestOverlayPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M && !Settings.canDrawOverlays(this)) {
            val intent = Intent(
                Settings.ACTION_MANAGE_OVERLAY_PERMISSION,
                Uri.parse("package:$packageName")
            )
            startActivityForResult(intent, REQUEST_OVERLAY)
        }
    }

    private fun startEditorService() {
        if (!Settings.canDrawOverlays(this)) {
            Toast.makeText(applicationContext, getString(R.string.permission_toast), Toast.LENGTH_LONG).show()
            requestOverlayPermission()
            return
        }
        val intent = Intent(this, EditorService::class.java)
            .setAction(EditorService.ACTION_START)
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) startForegroundService(intent) else startService(intent)
            Toast.makeText(applicationContext, getString(R.string.service_enabled), Toast.LENGTH_SHORT).show()
            mainHandler.postDelayed({ refreshConnectionStatus() }, 500)
        } catch (failure: Exception) {
            Toast.makeText(applicationContext, failure.message ?: "WayTouch could not start.", Toast.LENGTH_LONG).show()
        }
    }

    private fun matchWrap(): LinearLayout.LayoutParams =
        LinearLayout.LayoutParams(
            LinearLayout.LayoutParams.MATCH_PARENT,
            LinearLayout.LayoutParams.WRAP_CONTENT
        ).apply {
            bottomMargin = dp(12)
        }

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    companion object {
        private const val REQUEST_OVERLAY = 51
    }
}
