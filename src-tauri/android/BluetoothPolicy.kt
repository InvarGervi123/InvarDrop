package io.github.invargervi.invardrop

import java.util.UUID

internal object BluetoothPolicy {
    const val MAX_BOOTSTRAP = 512
    const val MAX_DEVICES = 32
    const val MAX_CONNECTIONS = 8
    const val SCAN_MILLIS = 24_000L
    const val PEER_MAX_AGE_MILLIS = 120_000L
    const val CONNECT_MILLIS = 35_000L
    const val SERVICE = "af8e6b90-6f6e-4e34-8a9e-4356fe9c3601"
    const val BOOTSTRAP = "af8e6b90-6f6e-4e34-8a9e-4356fe9c3602"
    const val REQUEST = "af8e6b90-6f6e-4e34-8a9e-4356fe9c3603"
    private val bootstrapRequiredFields = setOf("version", "id", "name", "service")
    private val bootstrapFields = bootstrapRequiredFields + "address"

    fun validBootstrapFields(fields: Set<String>): Boolean =
        fields == bootstrapFields || fields == bootstrapRequiredFields

    fun validUuid(value: String): Boolean = try {
        val uuid = UUID.fromString(value)
        uuid.version() == 4 && uuid.variant() == 2 && uuid.toString() == value
    } catch (_: IllegalArgumentException) { false }
    fun validAddress(value: String): Boolean =
        value.matches(Regex("[0-9A-Fa-f]{2}(:[0-9A-Fa-f]{2}){5}")) &&
            value != "00:00:00:00:00:00" && value != "02:00:00:00:00:00"
    fun validName(value: String): Boolean = value.isNotEmpty() && value.toByteArray(Charsets.UTF_8).size <= 80 &&
        value.none { it.isISOControl() || it in '\u200e'..'\u200f' || it in '\u202a'..'\u202e' || it in '\u2066'..'\u2069' }
    fun permissionAlias(sdk: Int): String = if (sdk >= 31) "nearbyBluetooth" else "legacyScan"
}
