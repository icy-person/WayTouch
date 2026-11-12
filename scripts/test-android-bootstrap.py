#!/usr/bin/env python3
"""Static contract test for the privileged shell bootstrap and private service."""
from pathlib import Path
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
A = "{http://schemas.android.com/apk/res/android}"
manifest = ET.parse(ROOT / "android-editor/src/main/AndroidManifest.xml").getroot()
application = manifest.find("application")
assert application is not None
activities = {item.get(A + "name"): item for item in application.findall("activity")}
service = application.find("service")
bootstrap = activities.get(".BootstrapActivity")
main_activity = activities.get(".MainActivity")

assert bootstrap is not None, "BootstrapActivity missing from manifest"
assert bootstrap.get(A + "exported") == "true"
assert bootstrap.get(A + "permission") == "android.permission.DUMP"
assert service is not None and service.get(A + "exported") == "false"
assert main_activity is not None and main_activity.get(A + "exported") == "true"

bootstrap_source = (ROOT / "android-editor/src/main/kotlin/com/waytouch/editor/BootstrapActivity.kt").read_text()
main_source = (ROOT / "android-editor/src/main/kotlin/com/waytouch/editor/MainActivity.kt").read_text()
service_source = (ROOT / "android-editor/src/main/kotlin/com/waytouch/editor/EditorService.kt").read_text()
start_script = (ROOT / "scripts/start-server.sh").read_text()
touch_server = (ROOT / "android-server/src/main/kotlin/com/waytouch/Server.kt").read_text()
mouse_server = (ROOT / "android-server/src/main/kotlin/com/waytouch/MouseInputServer.kt").read_text()
auth_source = (ROOT / "android-server/src/main/kotlin/com/waytouch/IpcAuth.kt").read_text()
layout_server = (ROOT / "android-server/src/main/kotlin/com/waytouch/LayoutConfigServer.kt").read_text()
rust_mapping = (ROOT / "rust-host/src/mapping.rs").read_text()
gui_source = (ROOT / "gui/waytouch.py").read_text()

assert 'Regex("[0-9a-fA-F]{64}")' in bootstrap_source
assert "EditorService.ACTION_START" in bootstrap_source
assert "EditorService.EXTRA_IPC_TOKEN" in bootstrap_source
assert "startForegroundService(serviceIntent)" in bootstrap_source
assert "Intent(this, EditorService::class.java)" in main_source
assert "EditorService.ACTION_START" in main_source
assert ".putString(IPC_TOKEN_KEY, ipcToken).apply()" in service_source
assert 'if (line == "PING")' in service_source
assert 'output.write("PONG\\n".toByteArray(Charsets.US_ASCII))' in service_source
assert 'output.write("OK CURSOR\\n".toByteArray(Charsets.US_ASCII))' in service_source
assert "handleCursorCommand(line)" in service_source
assert "private fun handleCursorCommand(line: String): Boolean" in service_source
assert "commandSucceeded = handleCursorCommand(line)" in service_source
assert 'output.write("OK CURSOR\\n".toByteArray(Charsets.US_ASCII))' in service_source
assert "private fun showCursor(x: Float, y: Float, style: String, size: Float, color: Int): Boolean" in service_source
assert "layout.buttons.size >= 9" not in service_source
assert "buttons.length() <= 9" not in layout_server
assert "self.buttons.len() + 1 > MAX_POINTERS" not in rust_mapping
assert "layout.buttons.len() > MAX_POINTERS - 1" not in rust_mapping
assert "len(buttons) >= 9" not in gui_source
assert "for (code in 1..767)" in service_source
assert "MAX_LINE_BYTES = 1024 * 1024" in layout_server
assert "readEditorBoundedLine(input, 1024 * 1024)" in service_source
assert "readBoundedLine(input, 1024 * 1024)" in (ROOT / "android-editor/src/main/kotlin/com/waytouch/editor/MainActivity.kt").read_text()
assert "WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE" in service_source
assert "WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE" in service_source
assert "WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS" in service_source
assert "val sidePx =" in service_source
assert """WindowManager.LayoutParams(
                sidePx,
                sidePx,""" in service_source
assert "view.setTarget(0.5f, 0.5f, style, size, color)" in service_source
assert "postInvalidateOnAnimation(left, top, right, bottom)" in service_source
assert "updateViewLayout(view, params)" in service_source
assert "cursorWindowParams" in service_source
assert "ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW" in service_source
assert "Android low-memory callback received" in service_source
# Health probes must not consume the persistent controller's exclusive slot.
assert service_source.index('if (line == "PING")') < service_source.index(
    "cursorClientActive.compareAndSet(false, true)"
)
assert 'output.write("PONG\\n".toByteArray(Charsets.US_ASCII))' in service_source
assert "private fun removeCursorImmediately()" in service_source
assert "HapticFeedbackConstants.CLOCK_TICK" in service_source
assert "WayTouchDesign.AMBER" in service_source
assert "snapGuideX" in service_source and "snapGuideY" in service_source
assert "private fun redoLastChange()" in service_source
assert "redoHistory.add(layout.toJson())" in service_source
assert "undoHistory.size > 50" in service_source
assert "redoHistory.clear()" in service_source
assert "private fun testEditorInGame(view: LayoutEditorView)" in service_source
assert "mainHandler.postDelayed(restore, 5_000L)" in service_source
assert "testRestoreRunnable?.let(mainHandler::removeCallbacks)" in service_source
assert "createEditorLayoutParams()" in service_source
assert "View.LAYOUT_DIRECTION_RTL" in service_source
assert "private val nudgeSteps = floatArrayOf(0.001f, 0.005f, 0.01f, 0.05f)" in service_source
assert "selectionBindingRect" in service_source
assert "selectionRemoveRect" in service_source
assert "nudgeStepLabels[nudgeStepIndex]" in service_source
assert "footerButtonBottom = height - dp(14f)" in service_source
assert "private fun showPresetPicker()" in service_source
assert "private fun confirmApplyGamePreset(preset: String)" in service_source
assert "private fun applyGamePreset(preset: String)" in service_source
assert "private fun toolbarIcon(label: String): String?" in service_source
assert 'label == "تست بازی" -> "▶"' in service_source
assert 'label == "ذخیره" -> "✓"' in service_source
assert "val delta = when" in service_source
assert 'EditorButton("fire", 0.94f, 0.42f, "mouse", 272)' in service_source
assert 'EditorButton("aim", 0.80f, 0.42f, "mouse", 273)' in service_source
assert 'EditorButton("skill_q", 0.78f, 0.68f, "key", 16)' in service_source
assert 'EditorButton("item_x", 0.76f, 0.88f, "key", 45)' in service_source
assert "چیدمان فعلی دکمه‌ها با الگوی انتخاب‌شده جایگزین می‌شود" in service_source
assert "adb shell am start -n com.waytouch.editor/.BootstrapActivity" in start_script
assert "--es com.waytouch.editor.IPC_TOKEN" in start_script
assert 'connection.sendall(b"PING\\n")' in start_script
assert 'reader.readline().strip() != "PONG"' in start_script
assert "WTCH_MOUSE_HEALTH_OK" in start_script
assert "adb shell am start-foreground-service -n com.waytouch.editor/.EditorService" not in start_script
assert 'setDisplayId", Int::class.javaPrimitiveType' in touch_server
assert "DEFAULT_DISPLAY_ID = 0" in touch_server
assert "prepareMotionEvent(event, InputDevice.SOURCE_TOUCHSCREEN)" in touch_server
assert "prepareMotionEvent(event, InputDevice.SOURCE_MOUSE)" in touch_server
assert "inputDeviceId(InputDevice.SOURCE_TOUCHSCREEN)" in touch_server
assert "inputDeviceId(InputDevice.SOURCE_MOUSE)" in touch_server
assert "INJECT_WAIT_FOR_FINISH" not in touch_server
assert "INJECT_WAIT_FOR_RESULT = 1" in touch_server
assert "clearStaleInjectedTouchState()" in touch_server
assert "WTCH_STARTUP_TOUCH_RECOVERY" in touch_server
assert "val injectMode = if (frame.action == MotionEvent.ACTION_MOVE)" in touch_server
assert "INJECT_WAIT_FOR_RESULT" in touch_server
assert "val injectMode = when (action)" in touch_server
assert "frame.action == MotionEvent.ACTION_MOVE" in touch_server
assert "WTCH_TOUCH_STATS accepted_moves=" in touch_server
assert "droppedMoves += 1L" in touch_server
assert "INJECT_ASYNC" in touch_server
assert 'TOKEN_FILE = "/data/local/tmp/waytouch-private/waytouch-ipc.token"' in (
    ROOT / "android-server/src/main/kotlin/com/waytouch/IpcAuth.kt"
).read_text()
assert 'REMOTE_TOKEN_DIR="/data/local/tmp/waytouch-private"' in start_script
assert "frame.action == MotionEvent.ACTION_MOVE" in touch_server
assert "frame.action != MotionEvent.ACTION_MOVE" in touch_server
assert "cursorConnectionSlots" in service_source
assert "cursorClientActive" in service_source
gui_source = (ROOT / "gui/waytouch.py").read_text()
rust_mouse = (ROOT / "rust-host/src/android_mouse.rs").read_text()
assert gui_source.count('connection.sendall(b"PING\\n")') >= 3
status_worker = gui_source.split("class _StatusProbeWorker", 1)[1].split("class WayTouchBackend", 1)[0]
assert "27183" not in status_worker, "periodic status probes must not compete for the exclusive touch controller"
assert "def _schedule_adb_recovery" in (ROOT / "gui/waytouch.py").read_text()
assert "def _schedule_server_recovery" in (ROOT / "gui/waytouch.py").read_text()
assert "workerPool.execute { serveCursorClient(client) }" in service_source
assert "serviceDestroyed = true" in service_source
assert "while (true)" in layout_server
assert 'if (line == "PING")' in layout_server
assert 'writeReply(output, "PONG")' in layout_server
assert "client.soTimeout = 5000" in layout_server
assert "var consumed = 0" in service_source
assert "consumed > maxBytes" in service_source
protocol = (ROOT / "android-server/src/main/kotlin/com/waytouch/Protocol.kt").read_text()
assert "if (next == 0L) 1L else next" in protocol
assert "state.lastSequence >= 0xffff_ffffL" in mouse_server
rust_input = (ROOT / "rust-host/src/input.rs").read_text()
rust_main = (ROOT / "rust-host/src/main.rs").read_text()
normalized_input = " ".join(rust_input.split())
assert "self.motion_counters.dropped.fetch_add(1, Ordering::Relaxed)" in normalized_input
assert "self.motion_counters .coalesced .fetch_add(1, Ordering::Relaxed)" in normalized_input
assert "input_dropped={input_dropped}" in rust_main
assert "tcp_dropped={tcp_dropped}" in rust_main
assert "WTCH_MOUSE_STATS accepted_motion=" in mouse_server
assert "droppedMotion += 1L" in mouse_server
assert "BufferedInputStream(client.getInputStream()" in mouse_server
assert "DROPPABLE_ACTIONS = setOf(3, 7, 8, 9, 10)" in mouse_server
assert "rawAction in DROPPABLE_ACTIONS" in mouse_server
assert "private val controllerActive = AtomicBoolean(false)" in mouse_server
assert "controllerActive.compareAndSet(false, true)" in mouse_server
assert 'line != "CONTROL"' in mouse_server
assert 'output.write("OK CONTROL\\n".toByteArray(Charsets.US_ASCII))' in mouse_server
assert 'if (line == "PING")' in mouse_server
assert 'output.write("PONG\\n".toByteArray(Charsets.US_ASCII))' in mouse_server
assert 'probeFields[2] == "13"' in mouse_server
assert "zeroPayload" in mouse_server
assert "writeAck(output, sequence)" in mouse_server
assert "ThreadPoolExecutor(\n        2, 2" in mouse_server
assert mouse_server.count("line = readBoundedLine(input, MAX_LINE_LENGTH) ?: break") == 4
assert 'connection.writer.write_all(b"CONTROL\\n")' in rust_mouse
assert 'response.trim_end_matches([\'\\r\', \'\\n\']) != "OK CONTROL"' in rust_mouse
assert 'FIELD_SEPARATOR = Regex("' in mouse_server
assert "split(FIELD_SEPARATOR)" in mouse_server
assert "MotionEvent.ACTION_BUTTON_PRESS" in mouse_server
assert "MotionEvent.ACTION_BUTTON_RELEASE" in mouse_server
assert "BufferedOutputStream(client.getOutputStream(), 8 * 1024)" in touch_server
assert "val input = socket.getInputStream()" in auth_source
assert "val input = socket.getInputStream()" in service_source

print("ANDROID_BOOTSTRAP_CONTRACT_OK activity=protected service=private token=64hex cursor_ack=true move_recovery=true mouse_frame_recovery=true")
