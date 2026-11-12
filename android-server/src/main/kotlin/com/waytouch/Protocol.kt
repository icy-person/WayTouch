package com.waytouch

import java.io.DataInputStream
import java.io.EOFException

internal const val WTCH_VERSION = 2
internal const val WTCH_MAX_POINTERS = 10

internal data class TouchPointer(
    val id: Int,
    val toolType: Int,
    val x: Float,
    val y: Float,
    val pressure: Float,
    val size: Float
)

internal data class TouchFrame(
    val version: Int,
    val action: Int,
    val pointerCount: Int,
    val actionIndex: Int,
    val sequence: Long,
    val pointers: List<TouchPointer>
) {
    companion object {
        fun read(input: DataInputStream, magic: ByteArray): TouchFrame {
            require(magic.contentEquals(byteArrayOf(0x57, 0x54, 0x43, 0x48))) { "invalid WTCH magic" }

            val version = input.readUnsignedByte()
            val action = input.readUnsignedByte()
            val count = input.readUnsignedByte()
            val actionIndex = input.readUnsignedByte()
            val sequence = Integer.toUnsignedLong(Integer.reverseBytes(input.readInt()))

            require(version == WTCH_VERSION) { "unsupported protocol version $version" }
            require(count in 1..WTCH_MAX_POINTERS) { "pointer count must be 1..$WTCH_MAX_POINTERS" }

            val pointers = ArrayList<TouchPointer>(count)
            repeat(count) {
                val id = input.readUnsignedByte()
                val toolType = input.readUnsignedByte()
                val reserved = input.readUnsignedShort()
                require(reserved == 0) { "reserved pointer bytes must be zero" }

                val x = readFloatLe(input)
                val y = readFloatLe(input)
                val pressure = readFloatLe(input)
                val size = readFloatLe(input)
                pointers += TouchPointer(id, toolType, x, y, pressure, size)
            }

            return TouchFrame(version, action, count, actionIndex, sequence, pointers)
        }

        private fun readFloatLe(input: DataInputStream): Float =
            Float.fromBits(Integer.reverseBytes(input.readInt()))
    }
}

internal class PointerState {
    private var active = emptyList<TouchPointer>()
    private var lastSequence: Long? = null
    private var gestureDownTime = 0L
    private var lastEventTime = 0L

    val pointers: List<TouchPointer> get() = active
    val isActive: Boolean get() = active.isNotEmpty()
    val downTime: Long get() = gestureDownTime

    fun nextEventTime(now: Long): Long = maxOf(now, lastEventTime)

    fun validate(frame: TouchFrame): List<TouchPointer> {
        require(frame.version == WTCH_VERSION)
        require(frame.pointerCount == frame.pointers.size)
        require(frame.pointerCount in 1..WTCH_MAX_POINTERS)
        require(frame.actionIndex in 0 until frame.pointerCount)

        val previous = lastSequence
        if (previous != null) {
            val next = (previous + 1L) and 0xffff_ffffL
            val expected = if (next == 0L) 1L else next
            require(frame.sequence == expected) {
                "out-of-order sequence: expected $expected, received ${frame.sequence}"
            }
        }

        val ids = frame.pointers.map { it.id }
        require(ids.distinct().size == ids.size) { "duplicate pointer IDs" }

        frame.pointers.forEach { pointer ->
            require(pointer.id in 0..31) { "pointer ID outside Android 0..31 range" }
            require(pointer.toolType == 1 || pointer.toolType == 2 || pointer.toolType == 4) {
                "unsupported tool type ${pointer.toolType}"
            }
            require(pointer.x.isFinite() && pointer.y.isFinite() && pointer.x >= 0f && pointer.y >= 0f) {
                "invalid pointer coordinates"
            }
            require(pointer.pressure.isFinite() && pointer.pressure in 0f..1f) { "invalid pressure" }
            require(pointer.size.isFinite() && pointer.size in 0f..1f) { "invalid size" }
        }

        when (frame.action) {
            0 -> {
                require(frame.actionIndex == 0 && frame.pointerCount == 1 && active.isEmpty()) {
                    "DOWN requires no active pointers and exactly one pointer"
                }
                return frame.pointers
            }
            5 -> {
                require(active.isNotEmpty() && frame.pointerCount == active.size + 1) {
                    "POINTER_DOWN must add exactly one pointer"
                }
                val newIndex = frame.actionIndex
                val oldIds = ids.toMutableList().also { it.removeAt(newIndex) }
                require(oldIds == active.map { it.id }) {
                    "POINTER_DOWN must preserve existing pointer order"
                }
                require(ids[newIndex] !in active.map { it.id }) { "new pointer ID is already active" }
                return frame.pointers
            }
            2 -> {
                require(frame.actionIndex == 0 && active.isNotEmpty() && ids == active.map { it.id }) {
                    "MOVE must contain the complete active pointer set in order"
                }
                return frame.pointers
            }
            6 -> {
                require(active.size >= 2 && ids == active.map { it.id }) {
                    "POINTER_UP must contain the complete active pointer set in order"
                }
                return frame.pointers.toMutableList().also { it.removeAt(frame.actionIndex) }
            }
            1 -> {
                require(frame.actionIndex == 0 && active.size == 1 && ids == active.map { it.id }) {
                    "UP requires the sole active pointer"
                }
                return emptyList()
            }
            3 -> {
                require(frame.actionIndex == 0 && active.isNotEmpty() && ids == active.map { it.id }) {
                    "CANCEL must contain the complete active pointer set"
                }
                return emptyList()
            }
            else -> throw IllegalArgumentException("unsupported MotionEvent action ${frame.action}")
        }
    }

    fun commit(frame: TouchFrame, nextActive: List<TouchPointer>, downTime: Long, eventTime: Long) {
        if (frame.action == 0) gestureDownTime = downTime
        active = nextActive
        lastSequence = frame.sequence
        lastEventTime = eventTime
        if (active.isEmpty()) gestureDownTime = 0L
    }

    fun resetAfterCancel() {
        active = emptyList()
        gestureDownTime = 0L
        lastEventTime = 0L
        lastSequence = null
    }
}
