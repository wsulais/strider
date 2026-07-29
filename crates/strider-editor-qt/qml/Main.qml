// SPDX-FileCopyrightText: 2026 Strider contributors
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// PROTOTYPE / THROWAWAY — the chrome, and only the chrome.
//
// Everything here is depth-insensitive interface furniture, which is what
// [[RFC-0006:C-OVERLAY]] 2 permits a host to composite. Note what is absent: nothing is
// anchored to a position in the cloud. A measurement label belongs in the renderer's draw
// list, because a composited one carries no depth and would float in front of geometry it
// should be behind ([[RFC-0006:C-OVERLAY]] 1) — and the renderer already draws those,
// depth-tested in hardware.
//
// The viewport is a native child window the renderer presents to, so QML cannot draw over
// it. The panel therefore sits beside it rather than on it: the "chrome around a viewport
// rectangle" arrangement [[ADR-0009]] accepts in exchange for no graphics interop.
import QtQuick
import QtQuick.Controls
import com.strider.editor
import com.strider.viewport

Window {
    id: root
    width: 1320
    height: 760
    visible: true
    color: "#0b0b0e"
    title: "strider — PROTOTYPE / THROWAWAY"

    property int panelWidth: 320

    Editor { id: editor }

    onClosing: editor.shutdown()

    // The frame loop, and it is deliberately here rather than in the renderer:
    // [[RFC-0006:C-RENDER]] 4 gives frame scheduling to the host, and in this application
    // the host is Qt. The renderer has no clock to start one with.
    // The frame loop, paced by the display rather than by a 16 ms timer.
    //
    // `FrameAnimation` fires once per rendered frame, so on a 144 Hz monitor the loop runs at
    // 144 Hz and not at the 60 Hz a hardcoded interval assumed. Frame scheduling still belongs
    // to the host ([[RFC-0006:C-RENDER]] 4) — this is the host doing it properly, by asking
    // the display instead of guessing.
    //
    // `update()` only on damage. The two requirements pull opposite ways and compose cleanly
    // once separated: damage decides WHETHER to draw, the display decides WHEN. A moving
    // camera is continuous damage and draws every vsync; a still one over a resident view
    // draws nothing, which matters more here than in most viewers because a paint is a
    // readback.
    // The frame loop, paced by real buffer swaps.
    //
    // Two earlier attempts got this wrong in opposite directions. A `Timer { interval: 0 }`
    // froze the application outright: a zero-interval repeating timer re-fires before the event
    // loop can service anything else. And `FrameAnimation` silently capped at 60, because Qt
    // drives it from the animation driver — a fixed 60 Hz timer — not from the display, so a
    // 144 Hz monitor reported 60 and it looked like a limit somewhere in the renderer.
    //
    // The viewport now asks for its own next frame from inside `frameSwapped`, which is the
    // display's actual rate and cannot starve the loop because the next swap gates it. Frame
    // scheduling still belongs to the host ([[RFC-0006:C-RENDER]] 4) — this is the host asking
    // the display instead of a timer that only resembles one.
    //
    // `Timer` remains only to advance host state when the viewport is idle, so retrieval still
    // completes with a still camera. It does not drive painting.
    Timer {
        interval: 8
        repeat: true
        running: true
        onTriggered: {
            editor.tick()
            // Unconditional. This used to be gated on `!viewport.continuous`, on the reasoning
            // that the swap-driven loop already covers the continuous case — which is true only
            // while swaps keep arriving, and says nothing about how the first one is provoked.
            // `update` on an item that is already dirty costs nothing, so the guard bought
            // nothing and lost the cold start.
            if (editor.needsPaint)
                viewport.update()
        }
    }

    Rectangle {
        id: panel
        width: root.panelWidth
        height: parent.height
        color: "#101014"

        Column {
            anchors.fill: parent
            anchors.margins: 14
            spacing: 3

            Text {
                text: "strider"
                color: "#e6e6ee"
                font.pixelSize: 17
                font.bold: true
            }
            Text {
                text: "PROTOTYPE / THROWAWAY"
                color: "#55555f"
                font.pixelSize: 10
            }
            Item { width: 1; height: 12 }

            // One string per line, "label\tvalue". A list of readouts rather than a
            // property each, so adding one is a Rust-side change only.
            Repeater {
                model: editor.readouts
                delegate: Row {
                    id: readoutRow
                    required property string modelData
                    spacing: 8
                    Text {
                        text: readoutRow.modelData.split("\t")[0]
                        color: "#8a8a95"
                        font.pixelSize: 11
                        width: 150
                    }
                    Text {
                        text: readoutRow.modelData.split("\t")[1] ?? ""
                        color: "#e6e6ee"
                        font.pixelSize: 11
                        font.bold: true
                    }
                }
            }

            Item { width: 1; height: 14 }

            // Controls. Depth-insensitive interface furniture, which is what
            // [[RFC-0006:C-OVERLAY]] 2 permits a host to composite — and it can now sit
            // anywhere, including over the viewport, because the viewport is a QML item.
            //
            // Two dropdowns rather than one list of every combination. The renderer's
            // `Shading` is a channel crossed with a ramp, and presenting it that way meant
            // changing the ramp also moved you to a different channel. The crossing happens
            // in Rust at the point of use.
            Text {
                text: "colour by"
                color: "#6ec6d6"
                font.pixelSize: 11
                font.bold: true
            }
            ComboBox {
                id: channelBox
                width: panel.width - 28
                height: 28
                model: editor.channelNames()
                currentIndex: editor.channelIndex
                onActivated: editor.setChannel(currentIndex)
            }

            Item { width: 1; height: 6 }
            Text {
                text: "ramp"
                color: editor.rampApplies ? "#6ec6d6" : "#3a3a44"
                font.pixelSize: 11
                font.bold: true
            }
            ComboBox {
                id: rampBox
                width: panel.width - 28
                height: 28
                model: editor.rampNames()
                currentIndex: editor.rampIndex
                // Greyed out when the choice would do nothing: the source's own colour is
                // already a colour, and classification is categorical, so a sequential ramp
                // over category numbers would imply an order that does not exist.
                enabled: editor.rampApplies
                onActivated: editor.setRamp(currentIndex)
            }

            Item { width: 1; height: 10 }
            Text {
                text: "rendering"
                color: "#6ec6d6"
                font.pixelSize: 11
                font.bold: true
            }
            CheckBox {
                // Continuous repaint, paced by the swap. Whether the swap itself is capped is
                // fixed when the window is created, so it is a startup choice
                // (STRIDER_NO_VSYNC=1) rather than something this can change mid-run.
                text: "continuous (pace to swaps)"
                checked: editor.vsyncOn
                onToggled: { editor.setVsync(checked); viewport.continuous = checked }
                font.pixelSize: 11
            }
            CheckBox {
                // "Adaptive", not "damage tracking": damage is the mechanism, and the choice
                // being offered is between repainting on change and repainting every frame.
                text: "adaptive (else immediate)"
                checked: editor.adaptiveOn
                onToggled: editor.setAdaptive(checked)
                font.pixelSize: 11
            }

            Item { width: 1; height: 12 }
            Text {
                text: "in the viewport:\n  drag — pan\n  wheel — zoom\n  shift+drag — reclassify gesture\n  0-9 — colour by\n  q w e r — ramp\n  u — undo"
                color: "#55555f"
                font.pixelSize: 10
                lineHeight: 1.5
            }
        }
    }

    // The viewport, now an ordinary QML item.
    //
    // It receives pointer and key events, holds focus, participates in layout, and can be
    // drawn over — none of which a native child window could do. That is the whole reason for
    // the swap: the input handling below replaces about 120 lines of C++ event forwarding.
    Viewport {
        id: viewport
        anchors.left: panel.right
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom

        // Focus so the keyboard arrives here. A child QWindow could not take it.
        focus: true
        continuous: editor.vsyncOn
        Keys.onPressed: (event) => { editor.keyPressed(event.key); event.accepted = true }

        MouseArea {
            id: pointer
            anchors.fill: parent
            acceptedButtons: Qt.LeftButton | Qt.MiddleButton | Qt.RightButton
            property real startX: 0
            property real startY: 0
            property bool editing: false

            onPressed: (mouse) => {
                viewport.forceActiveFocus()
                startX = mouse.x
                startY = mouse.y
                // Shift, or the right button, makes it an edit gesture. Both, because a
                // modifier is easy to miss and a second button is easy to explain.
                editing = (mouse.modifiers & Qt.ShiftModifier) || mouse.button === Qt.RightButton
                band.visible = editing
            }
            onPositionChanged: (mouse) => {
                if (editing) {
                    band.x = Math.min(startX, mouse.x)
                    band.y = Math.min(startY, mouse.y)
                    band.width = Math.abs(mouse.x - startX)
                    band.height = Math.abs(mouse.y - startY)
                } else if (pressed) {
                    editor.pan((startX - mouse.x) / width, (mouse.y - startY) / width)
                    startX = mouse.x
                    startY = mouse.y
                }
            }
            onReleased: (mouse) => {
                if (editing && Math.abs(mouse.x - startX) > 4 && Math.abs(mouse.y - startY) > 4) {
                    editor.lasso(Math.min(startX, mouse.x) / width,
                                 Math.min(startY, mouse.y) / height,
                                 Math.max(startX, mouse.x) / width,
                                 Math.max(startY, mouse.y) / height,
                                 false)
                }
                editing = false
                band.visible = false
            }
            onWheel: (wheel) => editor.zoom(wheel.angleDelta.y > 0 ? 0.86 : 1.16)
        }

        // The rubber band, which is now possible at all.
        //
        // Under the native-child-window arrangement QML could not draw over the viewport, so a
        // gesture had no feedback while it was being made and the band would have had to be
        // drawn by the renderer. It is depth-insensitive chrome, so compositing it is exactly
        // what [[RFC-0006:C-OVERLAY]] 2 permits — and it is now four lines.
        Rectangle {
            id: band
            visible: false
            color: "#3078c8ff"
            border.color: "#78c8ff"
            border.width: 1
        }
    }
}
