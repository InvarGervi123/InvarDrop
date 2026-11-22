package io.github.invargervi.invardrop

fun main() {
    val uuid = "c015841b-f164-4d98-b880-d9f0e1952bb4"
    check(BluetoothPolicy.validUuid(uuid))
    check(!BluetoothPolicy.validUuid(uuid.uppercase()))
    check(!BluetoothPolicy.validUuid("00000000-0000-0000-0000-000000000000"))
    check(!BluetoothPolicy.validUuid("c015841b-f164-4d98-7880-d9f0e1952bb4"))
    check(BluetoothPolicy.validAddress("AA:BB:CC:DD:EE:FF"))
    for (value in listOf("02:00:00:00:00:00", "00:00:00:00:00:00", "AA:BB:CC:DD:EE", "127.0.0.1")) check(!BluetoothPolicy.validAddress(value))
    check(BluetoothPolicy.validName("Nearby device"))
    check(!BluetoothPolicy.validName(""))
    check(!BluetoothPolicy.validName("x".repeat(81)))
    check(!BluetoothPolicy.validName("spoof\u202e"))
    check(!BluetoothPolicy.validName("line\nfeed"))
    check(BluetoothPolicy.permissionAlias(24) == "legacyScan")
    check(BluetoothPolicy.permissionAlias(30) == "legacyScan")
    check(BluetoothPolicy.permissionAlias(31) == "nearbyBluetooth")
    check(BluetoothPolicy.permissionAlias(37) == "nearbyBluetooth")
    val bootstrapFields = setOf("version", "id", "name", "service", "address")
    check(BluetoothPolicy.validBootstrapFields(bootstrapFields))
    check(BluetoothPolicy.validBootstrapFields(bootstrapFields - "address"))
    check(!BluetoothPolicy.validBootstrapFields(bootstrapFields + "trusted"))
    check(!BluetoothPolicy.validBootstrapFields(bootstrapFields - "address" + "trusted"))
    check(bootstrapFields.filter { it != "address" }.none { BluetoothPolicy.validBootstrapFields(bootstrapFields - it) })
    println("Bluetooth policy: 23 assertions passed")
}
