#!/usr/bin/env python3
"""Headless QML load test used by scripts/build.sh."""

import sys
from pathlib import Path

from PySide6.QtCore import QObject, QTimer, QUrl
from PySide6.QtGui import QGuiApplication
from PySide6.QtQml import QQmlApplicationEngine, QQmlEngine
from PySide6.QtQuickControls2 import QQuickStyle

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "gui"))

from waytouch import WayTouchBackend  # noqa: E402

QQuickStyle.setStyle("Material")
app = QGuiApplication([])
backend = WayTouchBackend(auto_probe=False)
QQmlEngine.setObjectOwnership(backend, QQmlEngine.CppOwnership)
engine = QQmlApplicationEngine()
backend.setParent(engine)
context = engine.rootContext()
context.setContextProperty("wayTouchApi", backend)
assert context.contextProperty("wayTouchApi") is backend
print("QML context API:", type(context.contextProperty("wayTouchApi")).__name__)
errors: list[str] = []
engine.warnings.connect(lambda warnings: errors.extend(str(item.toString()) for item in warnings))
engine.load(QUrl.fromLocalFile(str(ROOT / "gui" / "main.qml")))

if not engine.rootObjects():
    for error in errors:
        print(error, file=sys.stderr)
    raise SystemExit("Qt Quick failed to load gui/main.qml")

QTimer.singleShot(1200, app.quit)
app.exec()
if errors:
    for error in errors:
        print(error, file=sys.stderr)
    raise SystemExit("Qt Quick emitted QML warnings/errors")

root = engine.rootObjects()[0]
root.setProperty("width", 1180)
root.setProperty("height", 800)
panels = root.findChildren(QObject, "glassPanel")
heights = [float(panel.property("height")) for panel in panels]
if len(panels) != 20 or any(height < 60 for height in heights):
    raise SystemExit(f"Glass panel layout collapsed: count={len(panels)} heights={heights}")

for page in range(5):
    root.setProperty("pageIndex", page)
    app.processEvents()
    if int(root.property("pageIndex")) != page:
        raise SystemExit(f"Page navigation failed for index {page}")

print(
    f"QML_LAYOUT_OK size=1180x800 panels={len(panels)} "
    f"minHeight={min(heights):.1f} maxHeight={max(heights):.1f} pages=5"
)
print("QML_LOAD_OK roots=1 style=Material qt=6")

# Exercise real QObject notify bindings against the production backend.
from PySide6.QtQml import QQmlComponent  # noqa: E402

probe_component = QQmlComponent(engine)
probe_component.setData(
    b"""import QtQml
QtObject {
    property string lookMode: wayTouchApi.profile.mouse.look.mode
    property real buttonX: wayTouchApi.profile.buttons[0].position[0]
    property real buttonY: wayTouchApi.profile.buttons[0].position[1]
}
""",
    QUrl(),
)
probe = probe_component.create()
if probe is None:
    raise SystemExit("Reactive QML probe could not be created: " + "; ".join(
        item.toString() for item in probe_component.errors()
    ))

original_mode = str(backend.value("mouse.look.mode"))
next_mode = "native" if original_mode == "touch_drag" else "touch_drag"
backend.setValue("mouse.look.mode", next_mode)
app.processEvents()
if probe.property("lookMode") != next_mode:
    raise SystemExit("QML profile binding did not update after profileChanged.")
backend.setValue("mouse.look.mode", original_mode)

original_position = list(backend.value("buttons.0.position"))
updated_position = [
    min(0.99, max(0.01, float(original_position[0]) + 0.01)),
    min(0.99, max(0.01, float(original_position[1]) + 0.02)),
]
backend.updateAction(0, "position", updated_position)
app.processEvents()
actual_x = float(probe.property("buttonX"))
actual_y = float(probe.property("buttonY"))
if abs(actual_x - updated_position[0]) > 0.001 or abs(actual_y - updated_position[1]) > 0.001:
    raise SystemExit(
        f"Action position binding failed: expected={updated_position}, actual={[actual_x, actual_y]}"
    )
backend.updateAction(0, "position", original_position)
app.processEvents()
probe.deleteLater()
print("PROFILE_REACTIVE_BINDINGS_OK position_xy=true")
