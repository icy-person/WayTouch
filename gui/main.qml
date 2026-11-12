import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import QtQuick.Layouts
import QtQuick.Dialogs
import QtQuick.Effects

ApplicationWindow {
    id: window
    width: 1440
    height: 920
    minimumWidth: 1120
    minimumHeight: 760
    visible: true
    title: "WayTouch"
    color: "#0a111b"

    Material.theme: Material.Dark
    Material.accent: Material.Cyan
    Material.primary: Material.Cyan
    Material.background: "#111b28"

    property int pageIndex: 0

    function syncProfilePicker() {
        let selected = -1
        for (let i = 0; i < wayTouchApi.profileFiles.length; ++i) {
            if (wayTouchApi.profileFiles[i].path === wayTouchApi.profilePath)
                selected = i
        }
        if (selected >= 0 && profilePicker.currentIndex !== selected)
            profilePicker.currentIndex = selected
    }

    property var navItems: [
        { title: "Overview", icon: "◈" },
        { title: "Mouse & Look", icon: "⌖" },
        { title: "Touch layout", icon: "▦" },
        { title: "Input devices", icon: "⌨" },
        { title: "Diagnostics", icon: "⌁" }
    ]

    component GlassPanel: Rectangle {
        objectName: "glassPanel"
        radius: 20
        color: "#d9162433"
        border.width: 1
        border.color: "#35485d"
        antialiasing: true
        // Rectangle does not inherit its children's implicit size. Without
        // this, ColumnLayout treats most cards as zero-height and they overlap.
        implicitWidth: 280
        implicitHeight: children.length > 0
                       ? Math.max(80, children[0].implicitHeight + 36)
                       : 80
    }

    component Eyebrow: Label {
        color: "#8fa7be"
        font.pixelSize: 11
        font.weight: Font.DemiBold
        font.letterSpacing: 1.5
    }

    component PageHeading: ColumnLayout {
        property string heading: ""
        property string detail: ""
        spacing: 5
        Label {
            text: parent.heading
            color: "#f1f6fb"
            font.pixelSize: 25
            font.weight: Font.DemiBold
            Layout.fillWidth: true
        }
        Label {
            text: parent.detail
            color: "#91a5ba"
            font.pixelSize: 12
            wrapMode: Text.WordWrap
            Layout.fillWidth: true
        }
    }

    component StatusDot: Rectangle {
        property color dotColor: "#52d6a2"
        width: 9
        height: 9
        radius: 5
        color: dotColor
        antialiasing: true
    }

    background: Item {
        Rectangle {
            anchors.fill: parent
            gradient: Gradient {
                GradientStop { position: 0.0; color: "#09111d" }
                GradientStop { position: 0.52; color: "#101a29" }
                GradientStop { position: 1.0; color: "#0a111b" }
            }
        }

        Rectangle {
            id: glowA
            x: -160
            y: -210
            width: 600
            height: 600
            radius: 300
            opacity: 0.30
            gradient: Gradient {
                GradientStop { position: 0.0; color: "#195f83" }
                GradientStop { position: 0.58; color: "#15334f" }
                GradientStop { position: 1.0; color: "#00121e00" }
            }
            layer.enabled: true
            layer.effect: MultiEffect {
                blurEnabled: true
                blur: 0.92
                blurMax: 96
                autoPaddingEnabled: false
            }
        }
        Rectangle {
            id: glowB
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.rightMargin: -180
            anchors.bottomMargin: -230
            width: 640
            height: 640
            radius: 320
            opacity: 0.22
            gradient: Gradient {
                GradientStop { position: 0.0; color: "#176e69" }
                GradientStop { position: 0.62; color: "#153a4a" }
                GradientStop { position: 1.0; color: "#00121e00" }
            }
            layer.enabled: true
            layer.effect: MultiEffect {
                blurEnabled: true
                blur: 0.95
                blurMax: 110
                autoPaddingEnabled: false
            }
        }
    }

    RowLayout {
        anchors.fill: parent
        anchors.margins: 18
        spacing: 16

        // Left navigation rail
        GlassPanel {
            Layout.preferredWidth: 220
            Layout.fillHeight: true
            color: "#d9101926"

            ColumnLayout {
                anchors.fill: parent
                anchors.margins: 16
                spacing: 16

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 10
                    Rectangle {
                        width: 40
                        height: 40
                        radius: 13
                        color: "#1c3849"
                        border.color: "#47758a"
                        Text {
                            anchors.centerIn: parent
                            text: "W"
                            color: "#9ee5d7"
                            font.pixelSize: 23
                            font.weight: Font.Bold
                        }
                    }
                    ColumnLayout {
                        spacing: 0
                        Label {
                            text: "WAYTOUCH"
                            color: "#edf5fb"
                            font.pixelSize: 14
                            font.weight: Font.Bold
                            font.letterSpacing: 1.6
                        }
                        Label {
                            text: "WAYDROID INPUT"
                            color: "#7890a6"
                            font.pixelSize: 9
                            font.letterSpacing: 1.3
                        }
                    }
                }

                Rectangle {
                    Layout.fillWidth: true
                    height: 1
                    color: "#2b4052"
                }

                Eyebrow { text: "WORKSPACE" }

                Repeater {
                    model: window.navItems
                    delegate: Button {
                        required property var modelData
                        required property int index
                        Layout.fillWidth: true
                        Layout.preferredHeight: 46
                        checkable: true
                        checked: window.pageIndex === index
                        flat: true
                        onClicked: window.pageIndex = index

                        background: Rectangle {
                            radius: 13
                            color: checked ? "#263c4a" : (parent.hovered ? "#1b2b3a" : "transparent")
                            border.width: checked ? 1 : 0
                            border.color: "#466579"
                        }
                        contentItem: RowLayout {
                            spacing: 12
                            Label {
                                text: modelData.icon
                                color: checked ? "#9be4d6" : "#8399ae"
                                font.pixelSize: 18
                                Layout.preferredWidth: 25
                                horizontalAlignment: Text.AlignHCenter
                            }
                            Label {
                                text: modelData.title
                                color: checked ? "#f1f8fc" : "#9caec0"
                                font.pixelSize: 13
                                font.weight: checked ? Font.DemiBold : Font.Normal
                                Layout.fillWidth: true
                            }
                            Rectangle {
                                width: 5
                                height: 5
                                radius: 3
                                color: "#81dfc8"
                                visible: checked
                            }
                        }
                    }
                }

                Item { Layout.fillHeight: true }

                GlassPanel {
                    Layout.fillWidth: true
                    implicitHeight: 125
                    color: "#a51a2a39"
                    ColumnLayout {
                        anchors.fill: parent
                        anchors.margins: 13
                        spacing: 8
                        RowLayout {
                            Layout.fillWidth: true
                            StatusDot { dotColor: wayTouchApi.serverReady ? "#5bd6a3" : "#d99c55" }
                            Label {
                                text: wayTouchApi.serverReady ? "ANDROID READY" : "ANDROID OFFLINE"
                                color: wayTouchApi.serverReady ? "#8ce4be" : "#e8bd78"
                                font.pixelSize: 10
                                font.weight: Font.Bold
                                font.letterSpacing: 1.1
                            }
                        }
                        Label {
                            text: wayTouchApi.serverState
                            color: "#dce7f0"
                            font.pixelSize: 12
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                        Button {
                            text: "Refresh status"
                            flat: true
                            Layout.fillWidth: true
                            onClicked: wayTouchApi.refreshServerStatus()
                        }
                    }
                }

                Label {
                    text: "WayTouch  ·  Qt 6"
                    color: "#637b91"
                    font.pixelSize: 10
                    Layout.alignment: Qt.AlignHCenter
                }
            }
        }

        // Main glass workspace
        GlassPanel {
            Layout.fillWidth: true
            Layout.fillHeight: true
            color: "#c80e1723"

            ColumnLayout {
                anchors.fill: parent
                anchors.margins: 23
                spacing: 18

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 14

                    ColumnLayout {
                        spacing: 3
                        Label {
                            text: window.navItems[window.pageIndex].title
                            color: "#f1f6fb"
                            font.pixelSize: 23
                            font.weight: Font.DemiBold
                        }
                        Label {
                            text: "Precise input mapping and low-latency pointer routing"
                            color: "#8298ad"
                            font.pixelSize: 11
                        }
                    }

                    Item { Layout.fillWidth: true }

                    ComboBox {
                        id: profilePicker
                        Layout.preferredWidth: 190
                        textRole: "name"
                        model: wayTouchApi.profileFiles
                        currentIndex: {
                            let idx = -1
                            for (let i = 0; i < wayTouchApi.profileFiles.length; ++i) {
                                if (wayTouchApi.profileFiles[i].path === wayTouchApi.profilePath) idx = i
                            }
                            return idx
                        }
                        onActivated: function(index) {
                            if (index >= 0 && index < wayTouchApi.profileFiles.length) {
                                const loaded = wayTouchApi.loadProfile(wayTouchApi.profileFiles[index].path)
                                if (!loaded)
                                    Qt.callLater(window.syncProfilePicker)
                            }
                        }
                    }

                    Label {
                        text: wayTouchApi.profileName
                        color: "#8198ad"
                        font.pixelSize: 10
                    }
                    Button {
                        text: "Open"
                        flat: true
                        onClicked: openDialog.open()
                    }
                    Button {
                        text: "Save"
                        highlighted: wayTouchApi.dirty
                        onClicked: wayTouchApi.saveProfile()
                    }
                    Button {
                        text: "Save as"
                        flat: true
                        onClicked: saveDialog.open()
                    }
                }

                Rectangle {
                    Layout.fillWidth: true
                    height: 1
                    color: "#283c50"
                }

                StackLayout {
                    id: pages
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    currentIndex: window.pageIndex

                    // OVERVIEW
                    Flickable {
                        clip: true
                        contentWidth: width
                        contentHeight: overviewColumn.implicitHeight + 10
                        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

                        ColumnLayout {
                            id: overviewColumn
                            width: parent.width
                            spacing: 16

                            GlassPanel {
                                Layout.fillWidth: true
                                implicitHeight: 208
                                color: "#d11b303e"
                                RowLayout {
                                    anchors.fill: parent
                                    anchors.margins: 22
                                    spacing: 22

                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        spacing: 10
                                        Eyebrow { text: "CONTROL SESSION" }
                                        Label {
                                            text: wayTouchApi.controllerState === "Capturing" ? "Input capture active" :
                                                  wayTouchApi.controllerState === "Checking" ? "Checking Android connection…" :
                                                  wayTouchApi.controllerState === "Starting" ? "Starting controller…" :
                                                  wayTouchApi.controllerState === "Stopping" ? "Cancelling / releasing…" :
                                                  "Ready when you are"
                                            color: "#f4f8fb"
                                            font.pixelSize: 26
                                            font.weight: Font.DemiBold
                                        }
                                        Label {
                                            text: wayTouchApi.controllerState === "Checking"
                                                  ? "Running device, profile and Android checks in the background."
                                                  : wayTouchApi.controllerState === "Capturing"
                                                    ? "Keyboard and mouse are exclusively captured by WayTouch."
                                                    : "Start only after the Android bridge reports ready."
                                            color: "#9bb0c3"
                                            font.pixelSize: 12
                                            wrapMode: Text.WordWrap
                                            Layout.fillWidth: true
                                        }
                                        RowLayout {
                                            spacing: 9
                                            StatusDot { dotColor: wayTouchApi.controllerRunning ? "#5bd6a3" : "#7f91a4" }
                                            Label {
                                                text: wayTouchApi.controllerState.toUpperCase()
                                                color: wayTouchApi.controllerRunning ? "#89dfba" : "#9aabba"
                                                font.pixelSize: 10
                                                font.weight: Font.Bold
                                                font.letterSpacing: 1.1
                                            }
                                        }
                                    }

                                    Rectangle {
                                        Layout.preferredWidth: 1
                                        Layout.fillHeight: true
                                        color: "#3b5668"
                                    }

                                    ColumnLayout {
                                        Layout.preferredWidth: 220
                                        spacing: 10
                                        Button {
                                            text: "Start controller"
                                            icon.name: "media-playback-start"
                                            highlighted: true
                                            enabled: !wayTouchApi.controllerRunning && wayTouchApi.serverReady
                                            Layout.fillWidth: true
                                            onClicked: wayTouchApi.startControl()
                                        }
                                        Button {
                                            text: "Stop controller"
                                            icon.name: "media-playback-stop"
                                            enabled: wayTouchApi.controllerRunning
                                            Layout.fillWidth: true
                                            onClicked: wayTouchApi.stopControl(false)
                                        }
                                        Button {
                                            text: "Start / restart Android"
                                            flat: true
                                            enabled: !wayTouchApi.controllerRunning
                                            Layout.fillWidth: true
                                            onClicked: wayTouchApi.startAndroidServer()
                                        }
                                        Button {
                                            text: "Build / verify"
                                            flat: true
                                            enabled: !wayTouchApi.controllerRunning
                                            Layout.fillWidth: true
                                            onClicked: wayTouchApi.buildProject()
                                        }
                                    }
                                }
                            }

                            GridLayout {
                                Layout.fillWidth: true
                                columns: width > 850 ? 3 : 1
                                columnSpacing: 12
                                rowSpacing: 12

                                GlassPanel {
                                    Layout.fillWidth: true
                                    implicitHeight: 110
                                    ColumnLayout {
                                        anchors.fill: parent
                                        anchors.margins: 15
                                        spacing: 7
                                        Eyebrow { text: "ANDROID BRIDGE" }
                                        Label {
                                            text: wayTouchApi.serverState
                                            color: wayTouchApi.serverReady ? "#84e0b9" : "#e6b66d"
                                            font.pixelSize: 16
                                            font.weight: Font.DemiBold
                                            wrapMode: Text.WordWrap
                                        }
                                        Label { text: "ADB · WTCH v2 · TCP 27183"; color: "#8299ad"; font.pixelSize: 10 }
                                    }
                                }
                                GlassPanel {
                                    Layout.fillWidth: true
                                    implicitHeight: 110
                                    ColumnLayout {
                                        anchors.fill: parent
                                        anchors.margins: 15
                                        spacing: 7
                                        Eyebrow { text: "TOUCH ENGINE" }
                                        Label { text: "10 pointers"; color: "#eaf3f8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                        Label { text: "Multi-touch · WASD · action bindings"; color: "#8299ad"; font.pixelSize: 10 }
                                    }
                                }
                                GlassPanel {
                                    Layout.fillWidth: true
                                    implicitHeight: 110
                                    ColumnLayout {
                                        anchors.fill: parent
                                        anchors.margins: 15
                                        spacing: 7
                                        Eyebrow { text: "MOUSE ENGINE" }
                                        Label {
                                            text: !wayTouchApi.profile.mouse.enabled ? "Forwarding disabled" :
                                                  wayTouchApi.profile.mouse.look.mode === "touch_drag" ? "Touch Drag camera" : "Native relative mouse"
                                            color: "#eaf3f8"; font.pixelSize: 17; font.weight: Font.DemiBold
                                        }
                                        Label {
                                            text: "Camera lock: F3 only · Mouse mapping: F2"
                                            color: "#8299ad"; font.pixelSize: 10
                                        }
                                    }
                                }
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                implicitHeight: 120
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 17
                                    spacing: 8
                                    Eyebrow { text: "SAFETY" }
                                    Label {
                                        text: "Exclusive input capture is active only while the controller runs."
                                        color: "#e1edf5"
                                        font.pixelSize: 13
                                        font.weight: Font.Medium
                                    }
                                    Label {
                                        text: "GNOME will not receive captured keyboard, mouse or selected touchpad events. Emergency stop: Ctrl + Alt + Esc. Closing this app also terminates the controller."
                                        color: "#91a6ba"
                                        font.pixelSize: 11
                                        wrapMode: Text.WordWrap
                                        Layout.fillWidth: true
                                    }
                                }
                            }

                            Label {
                                text: wayTouchApi.statusMessage
                                color: "#8fa9bd"
                                wrapMode: Text.WordWrap
                                Layout.fillWidth: true
                                font.pixelSize: 11
                            }
                        }
                    }

                    // MOUSE & LOOK
                    Flickable {
                        clip: true
                        contentWidth: width
                        contentHeight: mouseColumn.implicitHeight + 10
                        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

                        ColumnLayout {
                            id: mouseColumn
                            width: parent.width
                            spacing: 15

                            PageHeading {
                                heading: "Mouse & Mouse Look"
                                detail: "Choose how the camera receives motion, then configure Aim and pointer visibility."
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                color: "#d11b303e"
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 17
                                    spacing: 11
                                    Eyebrow { text: "START HERE · RECOMMENDED PRESETS" }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        spacing: 10
                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 3
                                            Label { text: "Mobile shooter"; color: "#f2f7fb"; font.pixelSize: 14; font.weight: Font.DemiBold }
                                            Label {
                                                text: "Press F3 while the controller runs to lock/unlock Aim. F2 independently toggles mouse-button touch mapping."
                                                color: "#9bb0c3"; font.pixelSize: 11; wrapMode: Text.WordWrap; Layout.fillWidth: true
                                            }
                                        }
                                        Button {
                                            text: "Use recommended"
                                            highlighted: wayTouchApi.profile.mouse.look.mode !== "touch_drag" ||
                                                         Number(wayTouchApi.profile.mouse.look.hold_button) !== 273
                                            enabled: !wayTouchApi.controllerRunning
                                            onClicked: wayTouchApi.setMousePreset("mobile_shooter")
                                        }
                                    }
                                    Rectangle { Layout.fillWidth: true; height: 1; color: "#354b60" }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 3
                                            Label { text: "Native relative mouse"; color: "#e8f0f6"; font.pixelSize: 13; font.weight: Font.Medium }
                                            Label {
                                                text: "For games that explicitly capture raw relative mouse input. This is not a touch-camera fallback."
                                                color: "#8da3b8"; font.pixelSize: 11; wrapMode: Text.WordWrap; Layout.fillWidth: true
                                            }
                                        }
                                        Button {
                                            text: "Use native"
                                            flat: true
                                            enabled: !wayTouchApi.controllerRunning
                                            onClicked: wayTouchApi.setMousePreset("native_relative")
                                        }
                                    }
                                    Label {
                                        text: wayTouchApi.controllerRunning
                                              ? "Stop the controller before changing mouse presets."
                                              : "Preset changes are saved with the selected profile. Fine-tune sensitivity below."
                                        color: "#80d8bd"; font.pixelSize: 10; wrapMode: Text.WordWrap; Layout.fillWidth: true
                                    }
                                    Label {
                                        text: "Start the key mapper with the host mouse still free. F1 switches mouse capture to Android (Android cursor shown); press F1 again to restore host input. F2 toggles mouse-button mapping only; F3 controls camera aim."
                                        color: "#8fa5ba"; font.pixelSize: 10; wrapMode: Text.WordWrap; Layout.fillWidth: true
                                    }
                                }
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 18
                                    spacing: 13
                                    RowLayout {
                                        Layout.fillWidth: true
                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 2
                                            Label { text: "Native mouse forwarding"; color: "#edf4f9"; font.pixelSize: 14; font.weight: Font.Medium }
                                            Label { text: "Create the virtual evdev mouse and forward relative movement."; color: "#8da3b8"; font.pixelSize: 11; wrapMode: Text.WordWrap }
                                        }
                                        Switch {
                                            checked: wayTouchApi.profile.mouse.enabled
                                            onToggled: wayTouchApi.setValue("mouse.enabled", checked)
                                        }
                                    }
                                    Rectangle { Layout.fillWidth: true; height: 1; color: "#2d4052" }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 2
                                            Label { text: "Native mouse buttons"; color: "#edf4f9"; font.pixelSize: 14 }
                                            Label { text: "Forward left, right, middle, side and extra button transitions."; color: "#8da3b8"; font.pixelSize: 11; wrapMode: Text.WordWrap }
                                        }
                                        Switch {
                                            checked: wayTouchApi.profile.mouse.native_buttons
                                            onToggled: wayTouchApi.setValue("mouse.native_buttons", checked)
                                        }
                                    }
                                    Rectangle { Layout.fillWidth: true; height: 1; color: "#2d4052" }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 2
                                            Label { text: "Mouse Look"; color: "#edf4f9"; font.pixelSize: 14 }
                                            Label {
                                                text: wayTouchApi.profile.mouse.look.mode === "touch_drag"
                                                      ? "Mouse motion becomes a virtual Android finger; this works with touch-based mobile game cameras."
                                                      : "Mouse motion is injected through Android InputManager as SOURCE_MOUSE. The game must support native Android mouse input."
                                                color: "#8da3b8"; font.pixelSize: 11; wrapMode: Text.WordWrap
                                            }
                                        }
                                        Switch {
                                            checked: wayTouchApi.profile.mouse.look.enabled
                                            onToggled: wayTouchApi.setValue("mouse.look.enabled", checked)
                                        }
                                    }
                                    Rectangle { Layout.fillWidth: true; height: 1; color: "#2d4052" }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Camera control mode"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        ComboBox {
                                            model: ["Relative mouse", "Touch drag"]
                                            currentIndex: wayTouchApi.profile.mouse.look.mode === "touch_drag" ? 1 : 0
                                            onActivated: function(i) {
                                                wayTouchApi.setValue("mouse.look.mode", i === 1 ? "touch_drag" : "relative_mouse")
                                            }
                                            Layout.preferredWidth: 170
                                        }
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        ColumnLayout {
                                            Layout.fillWidth: true
                                            spacing: 2
                                            Label { text: "Consume the Aim button"; color: "#edf4f9"; font.pixelSize: 13 }
                                            Label { text: "Do not forward the hold button as a normal Android mouse click while Touch Drag is active."; color: "#8da3b8"; font.pixelSize: 11; wrapMode: Text.WordWrap }
                                        }
                                        Switch {
                                            checked: !wayTouchApi.profile.mouse.look.hold_button_passthrough
                                            onToggled: wayTouchApi.setValue("mouse.look.hold_button_passthrough", !checked)
                                            enabled: wayTouchApi.profile.mouse.look.mode === "touch_drag"
                                        }
                                    }
                                }
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 18
                                    spacing: 14
                                    Eyebrow { text: "POINTER RESPONSE" }

                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Cursor speed"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Label { text: Number(wayTouchApi.profile.mouse.cursor_speed).toFixed(2) + "×"; color: "#82dfc7"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                    }
                                    Slider {
                                        Layout.fillWidth: true
                                        from: 0.1; to: 5.0
                                        value: wayTouchApi.profile.mouse.cursor_speed
                                        onMoved: wayTouchApi.setValue("mouse.cursor_speed", Number(value.toFixed(2)))
                                    }
                                    Rectangle { Layout.fillWidth: true; height: 1; color: "#2d4052" }

                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Genre tuning"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Button { text: "FPS"; onClicked: wayTouchApi.setLookPreset("fps") }
                                        Button { text: "RPG"; onClicked: wayTouchApi.setLookPreset("rpg") }
                                        Button { text: "MOBA"; onClicked: wayTouchApi.setLookPreset("moba") }
                                    }
                                    Eyebrow { text: "CAMERA SENSITIVITY" }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Horizontal · X"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Label { text: Number(wayTouchApi.profile.mouse.look.sensitivity_x).toFixed(2); color: "#82dfc7"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                    }
                                    Slider {
                                        Layout.fillWidth: true
                                        from: 0.01; to: 10.0
                                        value: wayTouchApi.profile.mouse.look.sensitivity_x
                                        onMoved: wayTouchApi.setValue("mouse.look.sensitivity_x", Number(value.toFixed(2)))
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Vertical · Y"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Label { text: Number(wayTouchApi.profile.mouse.look.sensitivity_y).toFixed(2); color: "#82dfc7"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                    }
                                    Slider {
                                        Layout.fillWidth: true
                                        from: 0.01; to: 10.0
                                        value: wayTouchApi.profile.mouse.look.sensitivity_y
                                        onMoved: wayTouchApi.setValue("mouse.look.sensitivity_y", Number(value.toFixed(2)))
                                    }
                                    Rectangle { Layout.fillWidth: true; height: 1; color: "#2d4052" }

                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "EMA alpha · new-sample weight"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Label { text: Number(1.0 - wayTouchApi.profile.mouse.look.smoothing).toFixed(2); color: "#82dfc7"; font.pixelSize: 12 }
                                    }
                                    Slider {
                                        Layout.fillWidth: true
                                        from: 0.0; to: 0.95
                                        value: 1.0 - wayTouchApi.profile.mouse.look.smoothing
                                        onMoved: wayTouchApi.setValue("mouse.look.smoothing", Number((1.0 - value).toFixed(2)))
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Raw-count deadzone"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Label { text: Number(wayTouchApi.profile.mouse.look.deadzone).toFixed(1); color: "#82dfc7"; font.pixelSize: 12 }
                                    }
                                    Slider {
                                        Layout.fillWidth: true
                                        from: 0.0; to: 8.0
                                        value: wayTouchApi.profile.mouse.look.deadzone
                                        onMoved: wayTouchApi.setValue("mouse.look.deadzone", Number(value.toFixed(1)))
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "TouchDrag rate cap"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Label { text: String(wayTouchApi.profile.mouse.look.rate_cap_hz) + " Hz"; color: "#82dfc7"; font.pixelSize: 12 }
                                    }
                                    Slider {
                                        Layout.fillWidth: true
                                        from: 30; to: 500; stepSize: 10
                                        value: wayTouchApi.profile.mouse.look.rate_cap_hz
                                        onMoved: wayTouchApi.setValue("mouse.look.rate_cap_hz", Math.round(value))
                                        enabled: wayTouchApi.profile.mouse.look.mode === "touch_drag"
                                    }
                                    Rectangle { Layout.fillWidth: true; height: 1; color: "#2d4052" }

                                    Label {
                                        text: "F3 is the camera lock shortcut. F2 only enables/disables mapped mouse-button actions; disabled buttons pass through as native Android clicks."
                                        color: "#8fa5ba"; font.pixelSize: 10; wrapMode: Text.WordWrap; Layout.fillWidth: true
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Invert vertical axis"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Switch {
                                            checked: wayTouchApi.profile.mouse.look.invert_y
                                            onToggled: wayTouchApi.setValue("mouse.look.invert_y", checked)
                                        }
                                    }
                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        visible: wayTouchApi.profile.mouse.look.mode === "touch_drag"
                                        spacing: 9
                                        Rectangle { Layout.fillWidth: true; height: 1; color: "#2d4052" }
                                        Eyebrow { text: "TOUCH DRAG CAMERA REGION" }
                                        RowLayout {
                                            Layout.fillWidth: true
                                            Label { text: "Camera finger anchor · X"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                            Label { text: Number(wayTouchApi.profile.mouse.look.touch_anchor[0]).toFixed(2); color: "#82dfc7"; font.pixelSize: 11 }
                                        }
                                        Slider {
                                            Layout.fillWidth: true
                                            from: wayTouchApi.profile.mouse.look.touch_bounds[0]
                                            to: wayTouchApi.profile.mouse.look.touch_bounds[2]
                                            value: wayTouchApi.profile.mouse.look.touch_anchor[0]
                                            onMoved: wayTouchApi.setValue("mouse.look.touch_anchor.0", Number(value.toFixed(3)))
                                        }
                                        RowLayout {
                                            Layout.fillWidth: true
                                            Label { text: "Camera finger anchor · Y"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                            Label { text: Number(wayTouchApi.profile.mouse.look.touch_anchor[1]).toFixed(2); color: "#82dfc7"; font.pixelSize: 11 }
                                        }
                                        Slider {
                                            Layout.fillWidth: true
                                            from: wayTouchApi.profile.mouse.look.touch_bounds[1]
                                            to: wayTouchApi.profile.mouse.look.touch_bounds[3]
                                            value: wayTouchApi.profile.mouse.look.touch_anchor[1]
                                            onMoved: wayTouchApi.setValue("mouse.look.touch_anchor.1", Number(value.toFixed(3)))
                                        }
                                        RowLayout {
                                            Layout.fillWidth: true
                                            Label { text: "Left boundary"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                            Label { text: Number(wayTouchApi.profile.mouse.look.touch_bounds[0]).toFixed(2); color: "#82dfc7"; font.pixelSize: 11 }
                                        }
                                        Slider {
                                            Layout.fillWidth: true
                                            from: 0.0; to: Math.min(0.85, wayTouchApi.profile.mouse.look.touch_bounds[2] - 0.001, wayTouchApi.profile.mouse.look.touch_anchor[0])
                                            value: wayTouchApi.profile.mouse.look.touch_bounds[0]
                                            onMoved: wayTouchApi.setValue("mouse.look.touch_bounds.0", Number(value.toFixed(3)))
                                        }
                                        RowLayout {
                                            Layout.fillWidth: true
                                            Label { text: "Right boundary"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                            Label { text: Number(wayTouchApi.profile.mouse.look.touch_bounds[2]).toFixed(2); color: "#82dfc7"; font.pixelSize: 11 }
                                        }
                                        Slider {
                                            Layout.fillWidth: true
                                            from: Math.max(0.15, wayTouchApi.profile.mouse.look.touch_bounds[0] + 0.001, wayTouchApi.profile.mouse.look.touch_anchor[0])
                                            to: 1.0
                                            value: wayTouchApi.profile.mouse.look.touch_bounds[2]
                                            onMoved: wayTouchApi.setValue("mouse.look.touch_bounds.2", Number(value.toFixed(3)))
                                        }
                                        RowLayout {
                                            Layout.fillWidth: true
                                            Label { text: "Top / bottom limits"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                            Label { text: Number(wayTouchApi.profile.mouse.look.touch_bounds[1]).toFixed(2) + " / " + Number(wayTouchApi.profile.mouse.look.touch_bounds[3]).toFixed(2); color: "#82dfc7"; font.pixelSize: 11 }
                                        }
                                        RowLayout {
                                            Layout.fillWidth: true
                                            Label { text: "Top"; color: "#8da3b8"; font.pixelSize: 10 }
                                            Slider {
                                                Layout.fillWidth: true
                                                from: 0.0; to: Math.min(0.5, wayTouchApi.profile.mouse.look.touch_bounds[3] - 0.001, wayTouchApi.profile.mouse.look.touch_anchor[1])
                                                value: wayTouchApi.profile.mouse.look.touch_bounds[1]
                                                onMoved: wayTouchApi.setValue("mouse.look.touch_bounds.1", Number(value.toFixed(3)))
                                            }
                                            Label { text: "Bottom"; color: "#8da3b8"; font.pixelSize: 10 }
                                            Slider {
                                                Layout.fillWidth: true
                                                from: Math.max(0.5, wayTouchApi.profile.mouse.look.touch_bounds[1] + 0.001, wayTouchApi.profile.mouse.look.touch_anchor[1])
                                                to: 1.0
                                                value: wayTouchApi.profile.mouse.look.touch_bounds[3]
                                                onMoved: wayTouchApi.setValue("mouse.look.touch_bounds.3", Number(value.toFixed(3)))
                                            }
                                        }
                                        RowLayout {
                                            Layout.fillWidth: true
                                            Label { text: "Touch movement scale"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                            Label { text: Number(wayTouchApi.profile.mouse.look.touch_scale).toFixed(2) + "×"; color: "#82dfc7"; font.pixelSize: 11 }
                                        }
                                        RowLayout {
                                            Layout.fillWidth: true
                                            Label { text: "Recenter when camera finger reaches an edge"; color: "#e6eff6"; font.pixelSize: 11; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                                            Switch {
                                                checked: wayTouchApi.profile.mouse.look.edge_recenter
                                                onToggled: wayTouchApi.setValue("mouse.look.edge_recenter", checked)
                                            }
                                        }
                                        Slider {
                                            Layout.fillWidth: true; from: 0.1; to: 20.0
                                            value: wayTouchApi.profile.mouse.look.touch_scale
                                            onMoved: wayTouchApi.setValue("mouse.look.touch_scale", Number(value.toFixed(2)))
                                        }
                                        Label {
                                            text: "The virtual camera finger shares Android's ten-pointer limit with movement and action buttons. Keep room for the controls you use simultaneously."
                                            color: "#8fa5ba"; font.pixelSize: 10; wrapMode: Text.WordWrap; Layout.fillWidth: true
                                        }
                                    }
                                }
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 18
                                    spacing: 12
                                    Eyebrow { text: "ANDROID CUSTOM CURSOR" }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Show WayTouch cursor over Waydroid"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Switch {
                                            checked: wayTouchApi.profile.mouse.cursor.enabled
                                            onToggled: wayTouchApi.setValue("mouse.cursor.enabled", checked)
                                        }
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Cursor style"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                        ComboBox {
                                            model: ["Crosshair", "Pointer", "Dot"]
                                            currentIndex: ["crosshair", "pointer", "dot"].indexOf(wayTouchApi.profile.mouse.cursor.style)
                                            onActivated: function(i) { wayTouchApi.setValue("mouse.cursor.style", ["crosshair", "pointer", "dot"][i]) }
                                            Layout.preferredWidth: 150
                                            enabled: wayTouchApi.profile.mouse.cursor.enabled
                                        }
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Cursor size"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                        Label { text: Number(wayTouchApi.profile.mouse.cursor.size).toFixed(0) + " dp"; color: "#82dfc7"; font.pixelSize: 11 }
                                    }
                                    Slider {
                                        Layout.fillWidth: true; from: 12; to: 64
                                        value: wayTouchApi.profile.mouse.cursor.size
                                        onMoved: wayTouchApi.setValue("mouse.cursor.size", Number(value.toFixed(0)))
                                        enabled: wayTouchApi.profile.mouse.cursor.enabled
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Cursor color · #RRGGBB"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                        TextField {
                                            text: wayTouchApi.profile.mouse.cursor.color
                                            placeholderText: "#ff3158"
                                            maximumLength: 7
                                            validator: RegularExpressionValidator { regularExpression: /^#[0-9A-Fa-f]{6}$/ }
                                            onEditingFinished: wayTouchApi.setValue("mouse.cursor.color", text)
                                            Layout.preferredWidth: 130
                                            enabled: wayTouchApi.profile.mouse.cursor.enabled
                                        }
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Hide custom cursor while aiming"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                        Switch {
                                            checked: wayTouchApi.profile.mouse.cursor.hide_while_aiming
                                            onToggled: wayTouchApi.setValue("mouse.cursor.hide_while_aiming", checked)
                                            enabled: wayTouchApi.profile.mouse.cursor.enabled
                                        }
                                    }
                                    Label {
                                        text: "No desktop-shell extension is required. F1 captures the selected host input device and shows the Android cursor; host-pointer visibility remains controlled by the operating system."
                                        color: "#8fa5ba"; font.pixelSize: 10; wrapMode: Text.WordWrap; Layout.fillWidth: true
                                    }
                                    Label {
                                        text: "Android receives native SOURCE_MOUSE MotionEvents over TCP 27186. F3 diverts motion to a virtual touch finger; F1 alone controls host/Android mouse ownership."
                                        color: "#8fa5ba"; font.pixelSize: 10; wrapMode: Text.WordWrap; Layout.fillWidth: true
                                    }
                                }
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 18
                                    spacing: 12
                                    Eyebrow { text: "FILTERING · OPTIONAL" }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Smoothing"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Label { text: Number(wayTouchApi.profile.mouse.look.smoothing).toFixed(2); color: "#82dfc7"; font.pixelSize: 12 }
                                    }
                                    Slider {
                                        Layout.fillWidth: true
                                        from: 0.0; to: 0.95
                                        value: wayTouchApi.profile.mouse.look.smoothing
                                        onMoved: wayTouchApi.setValue("mouse.look.smoothing", Number(value.toFixed(2)))
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Deadzone"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Label { text: Number(wayTouchApi.profile.mouse.look.deadzone).toFixed(1) + " counts"; color: "#82dfc7"; font.pixelSize: 12 }
                                    }
                                    Slider {
                                        Layout.fillWidth: true
                                        from: 0.0; to: 8.0
                                        value: wayTouchApi.profile.mouse.look.deadzone
                                        onMoved: wayTouchApi.setValue("mouse.look.deadzone", Number(value.toFixed(1)))
                                    }
                                    Label {
                                        text: "For the lowest latency and most faithful aim, start with smoothing = 0 and deadzone = 0. Tune X and Y separately after testing in your game."
                                        color: "#8fa5ba"
                                        font.pixelSize: 11
                                        wrapMode: Text.WordWrap
                                        Layout.fillWidth: true
                                    }
                                    Rectangle { Layout.fillWidth: true; height: 1; color: "#2d4052" }
                                    Eyebrow { text: "ACCELERATION CURVE" }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Enable velocity acceleration"; color: "#e6eff6"; font.pixelSize: 12; Layout.fillWidth: true }
                                        Switch {
                                            checked: wayTouchApi.profile.mouse.look.acceleration.enabled
                                            onToggled: wayTouchApi.setValue("mouse.look.acceleration.enabled", checked)
                                        }
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Velocity threshold"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                        Label { text: Number(wayTouchApi.profile.mouse.look.acceleration.threshold).toFixed(2) + " counts/ms"; color: "#82dfc7"; font.pixelSize: 11 }
                                    }
                                    Slider {
                                        Layout.fillWidth: true; from: 0.05; to: 20.0
                                        value: wayTouchApi.profile.mouse.look.acceleration.threshold
                                        onMoved: wayTouchApi.setValue("mouse.look.acceleration.threshold", Number(value.toFixed(2)))
                                        enabled: wayTouchApi.profile.mouse.look.acceleration.enabled
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Maximum gain"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                        Label { text: Number(wayTouchApi.profile.mouse.look.acceleration.max_gain).toFixed(2) + "×"; color: "#82dfc7"; font.pixelSize: 11 }
                                    }
                                    Slider {
                                        Layout.fillWidth: true; from: 1.0; to: 5.0
                                        value: wayTouchApi.profile.mouse.look.acceleration.max_gain
                                        onMoved: wayTouchApi.setValue("mouse.look.acceleration.max_gain", Number(value.toFixed(2)))
                                        enabled: wayTouchApi.profile.mouse.look.acceleration.enabled
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Curve exponent"; color: "#e6eff6"; font.pixelSize: 11; Layout.fillWidth: true }
                                        Label { text: Number(wayTouchApi.profile.mouse.look.acceleration.exponent).toFixed(2); color: "#82dfc7"; font.pixelSize: 11 }
                                    }
                                    Slider {
                                        Layout.fillWidth: true; from: 0.25; to: 3.0
                                        value: wayTouchApi.profile.mouse.look.acceleration.exponent
                                        onMoved: wayTouchApi.setValue("mouse.look.acceleration.exponent", Number(value.toFixed(2)))
                                        enabled: wayTouchApi.profile.mouse.look.acceleration.enabled
                                    }
                                }
                            }
                        }
                    }

                    // TOUCH LAYOUT
                    Flickable {
                        clip: true
                        contentWidth: width
                        contentHeight: layoutColumn.implicitHeight + 10
                        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

                        ColumnLayout {
                            id: layoutColumn
                            width: parent.width
                            spacing: 15

                            PageHeading {
                                heading: "Touch layout"
                                detail: "WASD virtual joystick and multi-touch action buttons. All coordinates are normalized to the Android display."
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 18
                                    spacing: 12
                                    Eyebrow { text: "ANDROID DISPLAY" }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Width"; color: "#dce8f0"; font.pixelSize: 12 }
                                        TextField {
                                            text: String(wayTouchApi.profile.display.width)
                                            validator: IntValidator { bottom: 2; top: 8192 }
                                            onEditingFinished: wayTouchApi.setValue("display.width", Number(text))
                                            Layout.preferredWidth: 110
                                        }
                                        Label { text: "Height"; color: "#dce8f0"; font.pixelSize: 12 }
                                        TextField {
                                            text: String(wayTouchApi.profile.display.height)
                                            validator: IntValidator { bottom: 2; top: 8192 }
                                            onEditingFinished: wayTouchApi.setValue("display.height", Number(text))
                                            Layout.preferredWidth: 110
                                        }
                                        Item { Layout.fillWidth: true }
                                    }
                                    Rectangle {
                                        Layout.fillWidth: true
                                        height: 270
                                        radius: 16
                                        color: "#101923"
                                        border.color: "#354c61"
                                        Canvas {
                                            id: touchCanvas
                                            anchors.fill: parent
                                            anchors.margins: 14
                                            onPaint: {
                                                const ctx = getContext("2d")
                                                ctx.clearRect(0, 0, width, height)
                                                if (width <= 1 || height <= 1)
                                                    return
                                                const w = width
                                                const h = height
                                                const displayWidth = Number(wayTouchApi.profile.display.width)
                                                const displayHeight = Number(wayTouchApi.profile.display.height)
                                                const aspect = displayWidth / displayHeight
                                                if (!Number.isFinite(aspect) || aspect <= 0)
                                                    return
                                                let rw = w, rh = w / aspect
                                                if (rh > h) { rh = h; rw = h * aspect }
                                                const ox = (w - rw) / 2
                                                const oy = (h - rh) / 2
                                                ctx.fillStyle = "#172a39"
                                                ctx.fillRect(ox, oy, rw, rh)
                                                ctx.strokeStyle = "#617b90"
                                                ctx.lineWidth = 1
                                                ctx.strokeRect(ox, oy, rw, rh)

                                                const c = wayTouchApi.profile.movement.center
                                                const rawRadius = Number(wayTouchApi.profile.movement.radius) * Math.min(rw, rh)
                                                const radius = Number.isFinite(rawRadius) ? Math.max(1, rawRadius) : 1
                                                const cx = ox + Number(c[0]) * rw
                                                const cy = oy + Number(c[1]) * rh
                                                ctx.beginPath()
                                                ctx.arc(cx, cy, radius, 0, Math.PI * 2)
                                                ctx.fillStyle = "rgba(74, 169, 222, .16)"
                                                ctx.fill()
                                                ctx.strokeStyle = "#6bbbe5"
                                                ctx.stroke()
                                                ctx.beginPath()
                                                ctx.arc(cx, cy, 4, 0, Math.PI * 2)
                                                ctx.fillStyle = "#87d4ed"
                                                ctx.fill()

                                                const actions = wayTouchApi.profile.buttons || []
                                                for (let i = 0; i < actions.length; ++i) {
                                                    const p = actions[i].position
                                                    const x = ox + Number(p[0]) * rw
                                                    const y = oy + Number(p[1]) * rh
                                                    ctx.beginPath()
                                                    ctx.arc(x, y, 6, 0, Math.PI * 2)
                                                    ctx.fillStyle = "#70d6b0"
                                                    ctx.fill()
                                                }
                                            }
                                        }
                                    }
                                    Eyebrow { text: "MOVEMENT JOYSTICK" }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Movement keys"; color: "#dce8f0"; font.pixelSize: 11 }
                                        ComboBox {
                                            model: ["WASD", "Arrow keys", "Numpad"]
                                            currentIndex: {
                                                const keys = wayTouchApi.profile.movement.keys
                                                if (Number(keys.up) === 17 && Number(keys.left) === 30) return 0
                                                if (Number(keys.up) === 103 && Number(keys.left) === 105) return 1
                                                if (Number(keys.up) === 72 && Number(keys.left) === 75) return 2
                                                return 0
                                            }
                                            onActivated: function(index) {
                                                if (!wayTouchApi.setMovementPreset(index)) {
                                                    const keys = wayTouchApi.profile.movement.keys
                                                    if (Number(keys.up) === 17 && Number(keys.left) === 30) currentIndex = 0
                                                    else if (Number(keys.up) === 103 && Number(keys.left) === 105) currentIndex = 1
                                                    else if (Number(keys.up) === 72 && Number(keys.left) === 75) currentIndex = 2
                                                    else currentIndex = -1
                                                }
                                            }
                                        }
                                        Label {
                                            text: "۸ جهت با نرمال‌سازی حرکت مورب"
                                            color: "#8fa7bc"
                                            font.pixelSize: 10
                                        }
                                        Item { Layout.fillWidth: true }
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Center X"; color: "#dce8f0"; font.pixelSize: 11 }
                                        Slider {
                                            Layout.fillWidth: true
                                            from: 0; to: 1
                                            property real draftValue: wayTouchApi.profile.movement.center[0]
                                            property bool changedByUser: false
                                            value: draftValue
                                            onMoved: {
                                                draftValue = Number(value.toFixed(2))
                                                changedByUser = true
                                            }
                                            onPressedChanged: {
                                                if (!pressed && changedByUser) {
                                                    changedByUser = false
                                                    wayTouchApi.setValue("movement.center.0", draftValue)
                                                }
                                            }
                                        }
                                        Label { text: Number(wayTouchApi.profile.movement.center[0]).toFixed(2); color: "#8fe1cb"; font.pixelSize: 11 }
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Center Y"; color: "#dce8f0"; font.pixelSize: 11 }
                                        Slider {
                                            Layout.fillWidth: true
                                            from: 0; to: 1
                                            property real draftValue: wayTouchApi.profile.movement.center[1]
                                            property bool changedByUser: false
                                            value: draftValue
                                            onMoved: {
                                                draftValue = Number(value.toFixed(2))
                                                changedByUser = true
                                            }
                                            onPressedChanged: {
                                                if (!pressed && changedByUser) {
                                                    changedByUser = false
                                                    wayTouchApi.setValue("movement.center.1", draftValue)
                                                }
                                            }
                                        }
                                        Label { text: Number(wayTouchApi.profile.movement.center[1]).toFixed(2); color: "#8fe1cb"; font.pixelSize: 11 }
                                    }
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Label { text: "Radius"; color: "#dce8f0"; font.pixelSize: 11 }
                                        Slider {
                                            Layout.fillWidth: true
                                            from: 0.001; to: 0.5
                                            property real draftValue: wayTouchApi.profile.movement.radius
                                            property bool changedByUser: false
                                            value: draftValue
                                            onMoved: {
                                                draftValue = Number(value.toFixed(3))
                                                changedByUser = true
                                            }
                                            onPressedChanged: {
                                                if (!pressed && changedByUser) {
                                                    changedByUser = false
                                                    wayTouchApi.setValue("movement.radius", draftValue)
                                                }
                                            }
                                        }
                                        Label { text: Number(wayTouchApi.profile.movement.radius).toFixed(3); color: "#8fe1cb"; font.pixelSize: 11 }
                                    }
                                }
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 18
                                    spacing: 12
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Eyebrow { text: "ACTION BUTTONS" }
                                        Item { Layout.fillWidth: true }
                                        Button {
                                            text: "+ Add binding"
                                            highlighted: true
                                            onClicked: wayTouchApi.addAction()
                                        }
                                    }
                                    Label {
                                        text: "Each action maps one key or mouse button to a fixed virtual touch position."
                                        color: "#8fa5ba"
                                        font.pixelSize: 11
                                        wrapMode: Text.WordWrap
                                        Layout.fillWidth: true
                                    }
                                    Repeater {
                                        model: wayTouchApi.profile.buttons
                                        delegate: Rectangle {
                                            required property int index
                                            required property var modelData
                                            Layout.fillWidth: true
                                            implicitHeight: 106
                                            radius: 13
                                            color: "#9a203143"
                                            border.color: "#304b5e"
                                            RowLayout {
                                                anchors.fill: parent
                                                anchors.margins: 12
                                                spacing: 9
                                                ColumnLayout {
                                                    Layout.fillWidth: true
                                                    spacing: 7
                                                    RowLayout {
                                                        Layout.fillWidth: true
                                                        TextField {
                                                            text: modelData.name
                                                            placeholderText: "Action name"
                                                            Layout.fillWidth: true
                                                            onEditingFinished: wayTouchApi.updateAction(index, "name", text.trim())
                                                        }
                                                        ComboBox {
                                                            model: ["Keyboard", "Mouse"]
                                                            currentIndex: modelData.source === "mouse" ? 1 : 0
                                                            Layout.preferredWidth: 115
                                                            onActivated: function(i) { wayTouchApi.updateAction(index, "source", i === 1 ? "mouse" : "key") }
                                                        }
                                                        TextField {
                                                            text: String(modelData.code)
                                                            placeholderText: "Code"
                                                            validator: IntValidator {
                                                                bottom: modelData.source === "mouse" ? 272 : 0
                                                                top: modelData.source === "mouse" ? 287 : 767
                                                            }
                                                            Layout.preferredWidth: 70
                                                            onEditingFinished: {
                                                                wayTouchApi.updateAction(index, "code", Number(text))
                                                                text = String(modelData.code)
                                                            }
                                                        }
                                                    }
                                                    RowLayout {
                                                        Layout.fillWidth: true
                                                        Label { text: "X"; color: "#8fa7bc"; font.pixelSize: 10 }
                                                        Slider {
                                                            Layout.fillWidth: true
                                                            from: 0; to: 1
                                                            property real draftValue: modelData.position[0]
                                                            property bool changedByUser: false
                                                            value: draftValue
                                                            onMoved: {
                                                                draftValue = Number(value.toFixed(2))
                                                                changedByUser = true
                                                            }
                                                            onPressedChanged: {
                                                                if (!pressed && changedByUser) {
                                                                    changedByUser = false
                                                                    wayTouchApi.updateAction(index, "position", [draftValue, modelData.position[1]])
                                                                }
                                                            }
                                                        }
                                                        Label { text: "Y"; color: "#8fa7bc"; font.pixelSize: 10 }
                                                        Slider {
                                                            Layout.fillWidth: true
                                                            from: 0; to: 1
                                                            property real draftValue: modelData.position[1]
                                                            property bool changedByUser: false
                                                            value: draftValue
                                                            onMoved: {
                                                                draftValue = Number(value.toFixed(2))
                                                                changedByUser = true
                                                            }
                                                            onPressedChanged: {
                                                                if (!pressed && changedByUser) {
                                                                    changedByUser = false
                                                                    wayTouchApi.updateAction(index, "position", [modelData.position[0], draftValue])
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                Button {
                                                    text: "×"
                                                    flat: true
                                                    onClicked: wayTouchApi.removeAction(index)
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // DEVICES
                    Flickable {
                        clip: true
                        contentWidth: width
                        contentHeight: devicesColumn.implicitHeight + 10
                        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

                        ColumnLayout {
                            id: devicesColumn
                            width: parent.width
                            spacing: 15
                            PageHeading {
                                heading: "Input devices"
                                detail: "Stable udev links are preferred. Selected sources are grabbed exclusively while the controller is active."
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 18
                                    spacing: 11
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Eyebrow { text: "DEVICE DISCOVERY" }
                                        Item { Layout.fillWidth: true }
                                        Button {
                                            text: "Scan devices"
                                            onClicked: wayTouchApi.scanInputDevices()
                                        }
                                        Button {
                                            text: "Discover & configure"
                                            highlighted: true
                                            onClicked: wayTouchApi.discoverAndApplyInputDevices()
                                        }
                                    }
                                    Label {
                                        text: wayTouchApi.deviceSummary
                                        color: "#c2d1de"
                                        font.pixelSize: 11
                                        font.family: "monospace"
                                        wrapMode: Text.WrapAnywhere
                                        Layout.fillWidth: true
                                    }
                                }
                            }

                            component DevicePathEditor: ColumnLayout {
                                property string heading
                                property string role
                                property string paths
                                Layout.fillWidth: true
                                spacing: 7
                                Label { text: parent.heading; color: "#dfeaf2"; font.pixelSize: 13; font.weight: Font.DemiBold }
                                TextArea {
                                    id: editor
                                    Layout.fillWidth: true
                                    implicitHeight: 82
                                    wrapMode: TextEdit.NoWrap
                                    selectByMouse: true
                                    text: parent.paths
                                    placeholderText: "/dev/input/by-id/…"
                                    background: Rectangle {
                                        radius: 10
                                        color: "#aa0c1722"
                                        border.color: "#3b5267"
                                    }
                                }
                                Button {
                                    text: "Apply " + parent.heading
                                    flat: true
                                    onClicked: wayTouchApi.setDeviceList(parent.role, editor.text)
                                }
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 18
                                    spacing: 18
                                    DevicePathEditor {
                                        heading: "Keyboard devices"
                                        role: "keyboard"
                                        paths: (wayTouchApi.profile.devices.keyboard || []).join("\n")
                                    }
                                    Rectangle { Layout.fillWidth: true; height: 1; color: "#2d4052" }
                                    DevicePathEditor {
                                        heading: "Mouse devices"
                                        role: "mouse"
                                        paths: (wayTouchApi.profile.devices.mouse || []).join("\n")
                                    }
                                    Rectangle { Layout.fillWidth: true; height: 1; color: "#2d4052" }
                                    DevicePathEditor {
                                        heading: "Suppress-only devices"
                                        role: "suppress"
                                        paths: (wayTouchApi.profile.devices.suppress || []).join("\n")
                                    }
                                }
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 16
                                    spacing: 7
                                    Eyebrow { text: "IMPORTANT" }
                                    Label {
                                        text: "Every configured device must exist and be readable. The suppress-only list blocks devices without mapping them. Avoid eventN paths when a stable by-id or by-path link exists."
                                        color: "#91a7ba"
                                        font.pixelSize: 11
                                        wrapMode: Text.WordWrap
                                        Layout.fillWidth: true
                                    }
                                }
                            }
                        }
                    }

                    // DIAGNOSTICS
                    Flickable {
                        clip: true
                        contentWidth: width
                        contentHeight: diagnosticsColumn.implicitHeight + 10
                        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

                        ColumnLayout {
                            id: diagnosticsColumn
                            width: parent.width
                            spacing: 15
                            PageHeading {
                                heading: "Diagnostics"
                                detail: "The GUI uses the same Rust profile validator and process runtime as the command-line controller."
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 18
                                    spacing: 12
                                    RowLayout {
                                        Layout.fillWidth: true
                                        Button {
                                            text: "Validate profile"
                                            highlighted: true
                                            onClicked: wayTouchApi.validateProfile()
                                        }
                                        Button {
                                            text: "Check Android"
                                            flat: true
                                            onClicked: wayTouchApi.refreshServerStatus()
                                        }
                                        Button {
                                            text: "Test virtual mouse…"
                                            flat: true
                                            enabled: !wayTouchApi.controllerRunning
                                            onClicked: mouseTestDialog.open()
                                        }
                                        Item { Layout.fillWidth: true }
                                        Button {
                                            text: "Clear log"
                                            flat: true
                                            onClicked: wayTouchApi.clearLog()
                                        }
                                    }
                                    Label {
                                        text: wayTouchApi.statusMessage
                                        color: "#9db2c4"
                                        font.pixelSize: 11
                                        wrapMode: Text.WordWrap
                                        Layout.fillWidth: true
                                    }
                                    Label {
                                        text: wayTouchApi.latencySummary
                                        color: "#82dfc7"
                                        font.pixelSize: 11
                                        wrapMode: Text.WordWrap
                                        Layout.fillWidth: true
                                    }
                                }
                            }

                            GlassPanel {
                                Layout.fillWidth: true
                                Layout.fillHeight: true
                                implicitHeight: 430
                                ColumnLayout {
                                    anchors.fill: parent
                                    anchors.margins: 14
                                    spacing: 8
                                    Eyebrow { text: "LIVE ACTIVITY" }
                                    ScrollView {
                                        Layout.fillWidth: true
                                        Layout.fillHeight: true
                                        TextArea {
                                            readOnly: true
                                            selectByMouse: true
                                            wrapMode: TextEdit.Wrap
                                            text: wayTouchApi.logText
                                            color: "#bdd0de"
                                            font.family: "monospace"
                                            font.pixelSize: 10
                                            background: Rectangle { color: "transparent" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8
                    StatusDot { dotColor: wayTouchApi.controllerRunning ? "#63d8a7" : "#627b91" }
                    Label {
                        text: wayTouchApi.controllerState
                        color: "#92a9bd"
                        font.pixelSize: 10
                    }
                    Item { Layout.fillWidth: true }
                    Label {
                        text: wayTouchApi.dirty ? "UNSAVED CHANGES" : "PROFILE SAVED"
                        color: wayTouchApi.dirty ? "#e6b36d" : "#6f9b8d"
                        font.pixelSize: 9
                        font.weight: Font.Bold
                        font.letterSpacing: 1.1
                    }
                    Label { text: "·"; color: "#50677b" }
                    Label { text: "WTCH v2"; color: "#7891a7"; font.pixelSize: 10 }
                }
            }
        }
    }

    Dialog {
        id: mouseTestDialog
        title: "Test virtual mouse?"
        modal: true
        standardButtons: Dialog.Ok | Dialog.Cancel
        anchors.centerIn: parent
        width: 430
        Label {
            width: parent.width
            wrapMode: Text.WordWrap
            color: "#d9e6ef"
            text: "This sends a small pointer movement, right-button press/release and one wheel tick to the currently focused Wayland window. Run it only when that input is safe."
        }
        onAccepted: wayTouchApi.testVirtualMouse()
    }

    FileDialog {
        id: openDialog
        title: "Open WayTouch profile"
        fileMode: FileDialog.OpenFile
        nameFilters: ["WayTouch profiles (*.json)", "All files (*)"]
        onAccepted: wayTouchApi.loadProfile(selectedFile.toLocalFile())
    }

    FileDialog {
        id: saveDialog
        title: "Save WayTouch profile as"
        fileMode: FileDialog.SaveFile
        nameFilters: ["WayTouch profiles (*.json)"]
        onAccepted: wayTouchApi.saveProfileAs(selectedFile.toLocalFile())
    }

    Connections {
        target: wayTouchApi
        function onProfileChanged() {
            touchCanvas.requestPaint()
            window.syncProfilePicker()
        }
        function onProfileFilesChanged() { window.syncProfilePicker() }
    }

    onClosing: function(close) {
        if (wayTouchApi.controllerRunning) {
            close.accepted = false
            wayTouchApi.stopForQuit()
        }
    }
}
