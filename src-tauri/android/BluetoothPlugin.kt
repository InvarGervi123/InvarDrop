package io.github.invargervi.invardrop

import android.app.Activity
import android.bluetooth.*
import android.bluetooth.le.*
import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.ParcelUuid
import android.os.SystemClock
import android.util.Log
import androidx.annotation.Keep
import androidx.annotation.RequiresApi
import app.tauri.PermissionState
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin
import org.json.JSONObject
import java.io.FileInputStream
import java.io.FileOutputStream
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean

@InvokeArg
class StartBluetoothArgs { lateinit var bootstrap: String; lateinit var signal: Channel }
@InvokeArg
class BluetoothPermissionArgs { var request: Boolean = false }
@InvokeArg
class ConnectBluetoothArgs { lateinit var peer: String; lateinit var request: String }
@InvokeArg
class CancelBluetoothArgs { lateinit var request: String }

@Keep
@TauriPlugin(permissions = [
    Permission(strings = ["android.permission.BLUETOOTH_SCAN", "android.permission.BLUETOOTH_ADVERTISE", "android.permission.BLUETOOTH_CONNECT"], alias = "nearbyBluetooth"),
    Permission(strings = ["android.permission.ACCESS_FINE_LOCATION"], alias = "legacyScan")
])
class BluetoothPlugin(private val activity: Activity) : Plugin(activity) {
    private val handler = Handler(Looper.getMainLooper())
    private val enabled = AtomicBoolean(false)
    @Volatile private var generation = 0L
    @Volatile private var signal: Channel? = null
    private var adapter: BluetoothAdapter? = null
    private var server: BluetoothServerSocket? = null
    private var gattServer: BluetoothGattServer? = null
    private var localBytes = byteArrayOf()
    private var localService = UUID.randomUUID()
    private var gattRegistered = false
    private var advertiseCallback: AdvertiseCallback? = null
    private var scanCallback: ScanCallback? = null
    private var advertising = false
    @Volatile private var paused = false
    private var pausedAt = 0L
    @Volatile private var status = "disabled"
    @Volatile private var error: String? = null
    private data class Remote(val info: String, val sequence: Long, val observed: Long)
    private data class Incoming(val descriptor: ParcelFileDescriptor, val request: String)
    private val remotes = ConcurrentHashMap<String, Remote>()
    private val readDevices = ConcurrentHashMap.newKeySet<String>()
    private val clients = ConcurrentHashMap.newKeySet<BluetoothGatt>()
    private val sockets = ConcurrentHashMap.newKeySet<BluetoothSocket>()
    private val connecting = ConcurrentHashMap<String, BluetoothSocket>()
    private val incoming = java.util.ArrayDeque<Incoming>()
    private val failures = java.util.ArrayDeque<Map<String, String>>()
    private val handoffs = java.util.HashMap<String, ParcelFileDescriptor>()
    companion object {
        private const val TAG = "InvarDropBluetooth"
        private const val STARTUP_CALLBACK_MILLIS = 10_000L
        @Volatile private var active: BluetoothPlugin? = null
        // JNI takes ownership synchronously. No raw fd is sent through an
        // asynchronously cancellable Tauri response or accepted from JavaScript.
        @Keep
        @JvmStatic fun takeDescriptor(handle: String): Int {
            val plugin = active ?: return -1
            return synchronized(plugin.handoffs) {
                val descriptor = plugin.handoffs.remove(handle) ?: return@synchronized -1
                try { descriptor.detachFd() } catch (_: Exception) { try { descriptor.close() } catch (_: Exception) { }; -1 }
            }
        }
    }
    private var sequence = 0L
    private val mtus = ConcurrentHashMap<String, Int>()
    private val prepared = ConcurrentHashMap<String, ByteArray>()
    private var lastReverse = -10_000L
    private val reverseTokens = java.util.LinkedHashSet<String>()

    @Command
    fun permission(invoke: Invoke) {
        val alias = BluetoothPolicy.permissionAlias(Build.VERSION.SDK_INT)
        if (getPermissionState(alias) == PermissionState.GRANTED) invoke.resolveObject(mapOf("granted" to true))
        else if (invoke.parseArgs(BluetoothPermissionArgs::class.java).request) requestPermissionForAlias(alias, invoke, "permissionResult")
        else invoke.resolveObject(mapOf("granted" to false))
    }
    @PermissionCallback
    fun permissionResult(invoke: Invoke) {
        invoke.resolveObject(mapOf("granted" to (getPermissionState(BluetoothPolicy.permissionAlias(Build.VERSION.SDK_INT)) == PermissionState.GRANTED)))
    }
    private fun permitted(): Boolean = getPermissionState(BluetoothPolicy.permissionAlias(Build.VERSION.SDK_INT)) == PermissionState.GRANTED

    private class BluetoothFailure(val operation: String, val detail: String) : Exception()

    private inline fun <T> bluetoothStep(operation: String, action: () -> T): T = try {
        action().also { Log.i(TAG, "$operation succeeded") }
    } catch (failure: BluetoothFailure) {
        throw failure
    } catch (failure: Exception) {
        // Exception messages can contain Bluetooth addresses or serialized input.
        throw BluetoothFailure(operation, exceptionDetail(failure))
    }

    private fun exceptionDetail(failure: Exception): String {
        val type = failure.javaClass.simpleName
        val code = if (Build.VERSION.SDK_INT >= 34) SocketErrors.errorCode(failure) else null
        return if (code == null) type else "$type, Android status $code"
    }

    @RequiresApi(34)
    private object SocketErrors {
        fun errorCode(failure: Exception): Int? = (failure as? BluetoothSocketException)?.errorCode
    }

    private fun rfcommFailure(failure: BluetoothFailure): String {
        val message = "Android ${failure.operation} failed (${failure.detail})"
        Log.w(TAG, message)
        return message
    }

    private fun reportFailure(failure: BluetoothFailure) {
        if (status == "unavailable") return
        val message = "Android Bluetooth ${failure.operation} failed (${failure.detail})"
        Log.e(TAG, message)
        error = message
        status = "unavailable"
        changed()
    }

    private fun changed() { signal?.sendObject(true) }

    private fun updateStatus() {
        if (!enabled.get() || status == "unavailable") return
        val next = when {
            paused -> "paused"
            !advertising -> "starting"
            scanCallback != null -> "scanning"
            else -> "ready"
        }
        if (status != next) {
            status = next
            changed()
        }
    }

    private fun bootstrap(bytes: ByteArray): JSONObject {
        require(bytes.size in 1..BluetoothPolicy.MAX_BOOTSTRAP)
        val value = JSONObject(String(bytes, Charsets.UTF_8))
        require(BluetoothPolicy.validBootstrapFields(value.keys().asSequence().toSet()) && value.opt("version") == 1 &&
            BluetoothPolicy.validUuid(value.opt("id") as? String ?: "") &&
            BluetoothPolicy.validUuid(value.opt("service") as? String ?: "") &&
            BluetoothPolicy.validName(value.opt("name") as? String ?: ""))
        if (!value.isNull("address")) require(BluetoothPolicy.validAddress(value.opt("address") as? String ?: ""))
        return value
    }
    @Command
    fun start(invoke: Invoke) {
        try {
            shutdown()
            bluetoothStep("permission check") {
                if (!permitted()) throw BluetoothFailure("permission check", "Nearby devices or legacy scan permission is required")
            }
            val args = bluetoothStep("start argument decoding") { invoke.parseArgs(StartBluetoothArgs::class.java) }
            signal = args.signal
            val bytes = args.bootstrap.toByteArray(Charsets.UTF_8)
            val info = bluetoothStep("bootstrap validation") {
                bootstrap(bytes).also {
                    // Android uses reverse requests instead of publishing a Classic address.
                    require(it.isNull("address"))
                }
            }
            val manager = bluetoothStep("BluetoothManager lookup") {
                activity.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager
                    ?: throw BluetoothFailure("BluetoothManager lookup", "system service unavailable")
            }
            val radio = bluetoothStep("adapter lookup") {
                manager.adapter ?: throw BluetoothFailure("adapter lookup", "no Bluetooth adapter")
            }
            bluetoothStep("adapter power check") {
                if (!radio.isEnabled) throw BluetoothFailure("adapter power check", "enable Bluetooth in system settings")
            }
            localBytes = bytes
            localService = UUID.fromString(info.getString("service"))
            adapter = radio
            val listener = bluetoothStep("listenUsingRfcommWithServiceRecord") {
                radio.listenUsingRfcommWithServiceRecord("InvarDrop TLS", localService)
            }
            Log.i(TAG, "Secure RFCOMM listener and SDP service UUID $localService")
            server = listener
            enabled.set(true); active = this; generation++; val epoch = generation
            status = "starting"
            val gatt = bluetoothStep("openGattServer") {
                manager.openGattServer(activity, serverCallback(epoch))
                    ?: throw BluetoothFailure("openGattServer", "returned no GATT server")
            }
            gattServer = gatt
            bluetoothStep("BluetoothGattServer.addService") {
                val service = BluetoothGattService(UUID.fromString(BluetoothPolicy.SERVICE), BluetoothGattService.SERVICE_TYPE_PRIMARY)
                service.addCharacteristic(BluetoothGattCharacteristic(UUID.fromString(BluetoothPolicy.BOOTSTRAP), BluetoothGattCharacteristic.PROPERTY_READ, BluetoothGattCharacteristic.PERMISSION_READ))
                service.addCharacteristic(BluetoothGattCharacteristic(UUID.fromString(BluetoothPolicy.REQUEST), BluetoothGattCharacteristic.PROPERTY_WRITE, BluetoothGattCharacteristic.PERMISSION_WRITE))
                if (!gatt.addService(service)) throw BluetoothFailure("BluetoothGattServer.addService", "returned false")
            }
            handler.postDelayed({
                if (generation == epoch && !gattRegistered) {
                    reportFailure(BluetoothFailure("BluetoothGattServerCallback.onServiceAdded", "callback timed out"))
                }
            }, STARTUP_CALLBACK_MILLIS)
            bluetoothStep("RFCOMM listener thread startup") {
                Thread({
                    while (enabled.get() && generation == epoch) {
                        val socket = try { listener.accept() } catch (failure: Exception) {
                            handler.post {
                                if (generation == epoch) reportFailure(BluetoothFailure("BluetoothServerSocket.accept", failure.javaClass.simpleName))
                            }
                            break
                        }
                        if (!enabled.get() || generation != epoch || sockets.size >= BluetoothPolicy.MAX_CONNECTIONS) {
                            if (generation == epoch) Log.w(TAG, "Incoming RFCOMM socket rejected at listener capacity")
                            try { socket.close() } catch (_: Exception) { }
                            continue
                        }
                        sockets.add(socket)
                        Thread({ accept(socket, epoch) }, "InvarDrop-RFCOMM-accept").start()
                    }
                }, "InvarDrop-RFCOMM-listener").start()
            }
            scan()
            invoke.resolve()
        } catch (failure: BluetoothFailure) {
            shutdown()
            reportFailure(failure)
            invoke.reject(error, "bluetooth-startup")
        }
    }
    private fun advertise() {
        if (!enabled.get() || paused || !gattRegistered || advertiseCallback != null || status == "unavailable") return
        val callback = object : AdvertiseCallback() {
            override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
                if (advertiseCallback !== this) return
                Log.i(TAG, "AdvertiseCallback.onStartSuccess")
                advertising = true
                updateStatus()
            }
            override fun onStartFailure(errorCode: Int) {
                if (advertiseCallback === this) reportFailure(BluetoothFailure("AdvertiseCallback.onStartFailure", "Android status $errorCode"))
            }
        }
        advertiseCallback = callback
        try {
            val advertiser = bluetoothStep("getBluetoothLeAdvertiser") {
                adapter?.bluetoothLeAdvertiser ?: throw BluetoothFailure("getBluetoothLeAdvertiser", "returned no advertiser")
            }
            bluetoothStep("BluetoothLeAdvertiser.startAdvertising") {
                val settings = AdvertiseSettings.Builder().setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_POWER).setConnectable(true).setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM).build()
                val data = AdvertiseData.Builder().addServiceUuid(ParcelUuid(UUID.fromString(BluetoothPolicy.SERVICE))).setIncludeDeviceName(false).setIncludeTxPowerLevel(false).build()
                advertiser.startAdvertising(settings, data, callback)
            }
            handler.postDelayed({
                if (advertiseCallback === callback && !advertising) {
                    reportFailure(BluetoothFailure("AdvertiseCallback.onStartSuccess", "callback timed out"))
                }
            }, STARTUP_CALLBACK_MILLIS)
        } catch (failure: BluetoothFailure) { reportFailure(failure) }
    }
    private fun stopAdvertising() {
        val callback = advertiseCallback ?: return
        advertiseCallback = null
        advertising = false
        try { adapter?.bluetoothLeAdvertiser?.stopAdvertising(callback) } catch (_: Exception) { }
    }
    private fun serverCallback(epoch: Long) = object : BluetoothGattServerCallback() {
        override fun onServiceAdded(result: Int, service: BluetoothGattService) {
            handler.post {
                if (generation != epoch || status == "unavailable") return@post
                if (result == BluetoothGatt.GATT_SUCCESS) {
                    Log.i(TAG, "BluetoothGattServerCallback.onServiceAdded succeeded")
                    gattRegistered = true
                    advertise()
                } else {
                    reportFailure(BluetoothFailure("BluetoothGattServerCallback.onServiceAdded", "Android status $result"))
                }
            }
        }
        override fun onMtuChanged(device: BluetoothDevice, mtu: Int) { mtus[device.address] = mtu.coerceIn(23, 517) }
        override fun onConnectionStateChange(device: BluetoothDevice, result: Int, state: Int) {
            if (state == BluetoothProfile.STATE_DISCONNECTED) { mtus.remove(device.address); prepared.remove(device.address) }
        }
        override fun onCharacteristicReadRequest(device: BluetoothDevice, requestId: Int, offset: Int, characteristic: BluetoothGattCharacteristic) {
            val valid = enabled.get() && characteristic.uuid.toString() == BluetoothPolicy.BOOTSTRAP && offset in 0..localBytes.size
            val end = minOf(localBytes.size, offset + (mtus[device.address] ?: 23) - 1)
            gattServer?.sendResponse(device, requestId, if (valid) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_INVALID_OFFSET, offset, if (valid) localBytes.copyOfRange(offset, end) else null)
        }
        override fun onCharacteristicWriteRequest(device: BluetoothDevice, requestId: Int, characteristic: BluetoothGattCharacteristic, preparedWrite: Boolean, responseNeeded: Boolean, offset: Int, value: ByteArray) {
            var accepted = false
            try {
                require(enabled.get() && permitted() && characteristic.uuid.toString() == BluetoothPolicy.REQUEST && value.size in 1..BluetoothPolicy.MAX_BOOTSTRAP)
                if (preparedWrite) {
                    synchronized(prepared) {
                        require(prepared.size < 8 || prepared.containsKey(device.address))
                        val prefix = prepared[device.address] ?: byteArrayOf()
                        require(offset == prefix.size && prefix.size + value.size <= BluetoothPolicy.MAX_BOOTSTRAP)
                        prepared[device.address] = prefix + value
                    }
                    handler.postDelayed({ prepared.remove(device.address) }, 10_000)
                    accepted = true
                } else { require(offset == 0); reverseRequest(value); accepted = true }
            } catch (_: Exception) { prepared.remove(device.address) }
            if (responseNeeded) gattServer?.sendResponse(device, requestId, if (accepted) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_FAILURE, offset, if (preparedWrite && accepted) value else null)
        }
        override fun onExecuteWrite(device: BluetoothDevice, requestId: Int, execute: Boolean) {
            val value = prepared.remove(device.address)
            var accepted = !execute
            if (execute && value != null) try { reverseRequest(value); accepted = true } catch (_: Exception) { }
            gattServer?.sendResponse(device, requestId, if (accepted) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_FAILURE, 0, null)
        }
    }
    private fun reverseRequest(value: ByteArray) {
        require(enabled.get() && permitted() && value.size in 1..BluetoothPolicy.MAX_BOOTSTRAP)
        val request = JSONObject(String(value, Charsets.UTF_8))
        require(request.length() == 4 && request.getInt("version") == 1)
        val address = request.getString("address"); val service = request.getString("service"); val token = request.getString("token")
        require(BluetoothPolicy.validAddress(address) && BluetoothPolicy.validUuid(service) && BluetoothPolicy.validUuid(token))
        synchronized(reverseTokens) {
            val now = SystemClock.elapsedRealtime()
            require(now - lastReverse >= 5_000 && reverseTokens.size < 32 && reverseTokens.add(token))
            lastReverse = now
        }
        try {
            beginConnect(address, UUID.fromString(service), "", UUID.fromString(token))
        } catch (failure: BluetoothFailure) {
            rfcommFailure(failure)
            throw failure
        }
    }
    @Command
    fun scan(invoke: Invoke) {
        if (!enabled.get() || !permitted() || paused || status == "unavailable") {
            invoke.reject("Bluetooth scan is unavailable")
            return
        }
        try { scan(); invoke.resolve() }
        catch (failure: BluetoothFailure) { reportFailure(failure); invoke.reject(error, "bluetooth-startup") }
    }
    private fun scan() {
        if (paused || scanCallback != null) return
        readDevices.clear()
        val scanner = bluetoothStep("getBluetoothLeScanner") {
            adapter?.bluetoothLeScanner ?: throw BluetoothFailure("getBluetoothLeScanner", "returned no scanner")
        }
        val callback = newScanCallback()
        bluetoothStep("BluetoothLeScanner.startScan") {
            val filter = ScanFilter.Builder().setServiceUuid(ParcelUuid(UUID.fromString(BluetoothPolicy.SERVICE))).build()
            scanner.startScan(listOf(filter), ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_POWER).build(), callback)
        }
        scanCallback = callback
        updateStatus()
        handler.postDelayed({ if (scanCallback === callback) stopScan() }, BluetoothPolicy.SCAN_MILLIS)
    }
    private fun stopScan() {
        val callback = scanCallback ?: return
        scanCallback = null
        try { adapter?.bluetoothLeScanner?.stopScan(callback) } catch (_: Exception) { }
        updateStatus()
    }
    private fun newScanCallback() = object : ScanCallback() {
        override fun onScanFailed(errorCode: Int) {
            if (scanCallback === this) reportFailure(BluetoothFailure("ScanCallback.onScanFailed", "Android status $errorCode"))
        }
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            if (scanCallback !== this || status == "unavailable" || clients.size >= 2 || readDevices.size >= BluetoothPolicy.MAX_DEVICES) return
            val device = result.device
            if (!readDevices.add(device.address)) return
            try {
                val gatt = device.connectGatt(activity, false, clientCallback, BluetoothDevice.TRANSPORT_LE)
                clients.add(gatt)
                handler.postDelayed({ if (clients.remove(gatt)) { try { gatt.disconnect(); gatt.close() } catch (_: Exception) { } } }, 10_000)
            } catch (_: Exception) { }
        }
    }
    private val clientCallback = object : BluetoothGattCallback() {
        override fun onConnectionStateChange(gatt: BluetoothGatt, result: Int, state: Int) {
            if (result == BluetoothGatt.GATT_SUCCESS && state == BluetoothProfile.STATE_CONNECTED && enabled.get()) gatt.discoverServices()
            else if (state == BluetoothProfile.STATE_DISCONNECTED || result != BluetoothGatt.GATT_SUCCESS) closeGatt(gatt)
        }
        override fun onServicesDiscovered(gatt: BluetoothGatt, result: Int) {
            val characteristic = gatt.getService(UUID.fromString(BluetoothPolicy.SERVICE))?.getCharacteristic(UUID.fromString(BluetoothPolicy.BOOTSTRAP))
            if (result == BluetoothGatt.GATT_SUCCESS && characteristic != null) gatt.readCharacteristic(characteristic) else closeGatt(gatt)
        }
        @Deprecated("Compatibility callback for Android before API 33")
        override fun onCharacteristicRead(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic, result: Int) { received(gatt, characteristic.uuid, characteristic.value ?: byteArrayOf(), result) }
        override fun onCharacteristicRead(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic, value: ByteArray, result: Int) { received(gatt, characteristic.uuid, value, result) }
    }
    private fun received(gatt: BluetoothGatt, uuid: UUID, value: ByteArray, result: Int) {
        try {
            require(enabled.get() && !paused && result == BluetoothGatt.GATT_SUCCESS && uuid.toString() == BluetoothPolicy.BOOTSTRAP)
            val info = bootstrap(value)
            if (remotes.size < BluetoothPolicy.MAX_DEVICES || remotes.containsKey(info.getString("id"))) {
                synchronized(this) {
                    sequence++
                    remotes[info.getString("id")] = Remote(info.toString(), sequence, SystemClock.elapsedRealtime())
                }
                changed()
            }
        } catch (_: Exception) { } finally { closeGatt(gatt) }
    }
    private fun closeGatt(gatt: BluetoothGatt) {
        clients.remove(gatt)
        try { gatt.disconnect(); gatt.close() } catch (_: Exception) { }
    }
    @Command
    fun connect(invoke: Invoke) {
        try {
            val args = bluetoothStep("RFCOMM connect command decoding") {
                invoke.parseArgs(ConnectBluetoothArgs::class.java).also { require(BluetoothPolicy.validUuid(it.request)) }
            }
            val info = bluetoothStep("RFCOMM peer route lookup") {
                val remote = remotes[args.peer]
                    ?: throw BluetoothFailure("RFCOMM peer route lookup", "peer unavailable, scan again")
                JSONObject(remote.info).also {
                    if (it.isNull("address")) throw BluetoothFailure("RFCOMM peer route lookup", "peer has no Classic locator")
                }
            }
            beginConnect(info.getString("address"), UUID.fromString(info.getString("service")), args.request, null)
            invoke.resolve()
        } catch (failure: BluetoothFailure) { invoke.reject(rfcommFailure(failure), "bluetooth-rfcomm") }
    }
    @Synchronized
    private fun beginConnect(address: String, service: UUID, request: String, reverse: UUID?) {
        val direction = if (reverse == null) "direct" else "reverse"
        Log.i(TAG, "Outgoing $direction RFCOMM target SDP UUID $service")
        val radio = bluetoothStep("outgoing RFCOMM preflight") {
            if (!enabled.get() || !permitted()) throw BluetoothFailure("outgoing RFCOMM preflight", "sharing disabled or permission missing")
            if (sockets.size >= BluetoothPolicy.MAX_CONNECTIONS || connecting.isNotEmpty()) {
                throw BluetoothFailure("outgoing RFCOMM preflight", "connection limit reached")
            }
            checkNotNull(adapter)
        }
        bluetoothStep("outgoing RFCOMM discovery stop") { stopScan(); radio.cancelDiscovery() }
        val device = bluetoothStep("outgoing RFCOMM Classic device lookup") { radio.getRemoteDevice(address) }
        val socket = bluetoothStep("createRfcommSocketToServiceRecord") { device.createRfcommSocketToServiceRecord(service) }
        sockets.add(socket)
        val key = if (request.isEmpty()) reverse.toString() else request
        connecting[key] = socket
        val epoch = generation
        val timeout = Runnable {
            if (connecting.remove(key, socket)) {
                closeSocket(socket)
                if (generation == epoch) failConnect(request, BluetoothFailure("outgoing $direction RFCOMM socket or preface", "timed out; check any OS pairing prompt"))
            }
        }
        handler.postDelayed(timeout, BluetoothPolicy.CONNECT_MILLIS)
        Thread({
            var activeAttempt = false
            var handedOff = false
            try {
                val bondBefore = bondState(device)
                Log.i(TAG, "Outgoing $direction BluetoothSocket.connect starting, bond state $bondBefore")
                bluetoothStep("outgoing $direction BluetoothSocket.connect") {
                    try { socket.connect() } catch (failure: Exception) {
                        throw BluetoothFailure("outgoing $direction BluetoothSocket.connect", "${exceptionDetail(failure)}, bond $bondBefore to ${bondState(device)}")
                    }
                }
                Log.i(TAG, "Outgoing $direction RFCOMM connected, bond state ${bondState(device)}")
                val preface = ByteArray(25)
                "IVDBT001".toByteArray(Charsets.US_ASCII).copyInto(preface)
                if (reverse != null) {
                    preface[8] = 1
                    java.nio.ByteBuffer.wrap(preface, 9, 16).putLong(reverse.mostSignificantBits).putLong(reverse.leastSignificantBits)
                }
                bluetoothStep("outgoing $direction RFCOMM routing preface write") {
                    socket.outputStream.write(preface)
                    socket.outputStream.flush()
                }
                activeAttempt = connecting.remove(key, socket)
                if (!activeAttempt || !enabled.get() || generation != epoch) return@Thread
                bluetoothStep("outgoing $direction RFCOMM descriptor handoff queue") { bridge(socket, request, epoch) }
                handedOff = true
            } catch (failure: BluetoothFailure) {
                if (generation == epoch && (activeAttempt || connecting.remove(key, socket))) failConnect(request, failure)
            } finally {
                handler.removeCallbacks(timeout)
                if (!handedOff) closeSocket(socket)
            }
        }, "InvarDrop-RFCOMM-connect").start()
    }

    private fun bondState(device: BluetoothDevice): String = try {
        when (device.bondState) {
            BluetoothDevice.BOND_NONE -> "none"
            BluetoothDevice.BOND_BONDING -> "bonding"
            BluetoothDevice.BOND_BONDED -> "bonded"
            else -> "unknown"
        }
    } catch (_: SecurityException) { "unavailable" }

    private fun failConnect(request: String, failure: BluetoothFailure) {
        val detail = rfcommFailure(failure)
        if (request.isNotEmpty()) {
            synchronized(failures) {
                if (failures.size < BluetoothPolicy.MAX_CONNECTIONS) failures.add(mapOf("request" to request, "error" to detail))
            }
            changed()
        }
    }

    private fun closeSocket(socket: BluetoothSocket) {
        sockets.remove(socket)
        try { socket.close() } catch (_: Exception) { }
    }

    private fun accept(socket: BluetoothSocket, epoch: Long) {
        Log.i(TAG, "Incoming RFCOMM socket accepted")
        val timedOut = AtomicBoolean(false)
        val timeout = Runnable { timedOut.set(true); closeSocket(socket) }
        handler.postDelayed(timeout, 3_000)
        try {
            val bytes = bluetoothStep("incoming RFCOMM routing preface read") {
                val bytes = ByteArray(25)
                var offset = 0
                while (offset < bytes.size) {
                    val count = socket.inputStream.read(bytes, offset, bytes.size - offset)
                    if (count <= 0) throw BluetoothFailure("incoming RFCOMM routing preface read", "socket closed before the preface was complete")
                    offset += count
                }
                bytes
            }
            bluetoothStep("incoming RFCOMM routing preface validation") {
                require(bytes.copyOfRange(0, 8).contentEquals("IVDBT001".toByteArray(Charsets.US_ASCII)) && bytes[8] == 0.toByte() && bytes.copyOfRange(9, 25).all { it == 0.toByte() })
            }
            handler.removeCallbacks(timeout)
            if (timedOut.get()) throw BluetoothFailure("incoming RFCOMM routing preface read", "timed out")
            bluetoothStep("incoming RFCOMM descriptor handoff queue") { bridge(socket, "", epoch) }
        } catch (failure: BluetoothFailure) {
            if (generation == epoch) rfcommFailure(if (timedOut.get()) BluetoothFailure("incoming RFCOMM routing preface read", "timed out") else failure)
            closeSocket(socket)
        }
        finally { handler.removeCallbacks(timeout) }
    }
    private fun bridge(socket: BluetoothSocket, request: String, epoch: Long) {
        val pair = ParcelFileDescriptor.createSocketPair()
        val closed = AtomicBoolean(false)
        fun close() {
            if (closed.compareAndSet(false, true)) {
                closeSocket(socket)
                try { pair[1].close() } catch (_: Exception) { }
            }
        }
        synchronized(incoming) {
            if (!enabled.get() || generation != epoch || incoming.size + synchronized(handoffs) { handoffs.size } >= BluetoothPolicy.MAX_CONNECTIONS) {
                pair[0].close()
                close()
                throw BluetoothFailure("RFCOMM descriptor handoff queue", "sharing stopped or queue limit reached")
            }
            incoming.add(Incoming(pair[0], request))
        }
        changed()
        // Only encrypted TLS bytes cross this bounded kernel socketpair. No
        // file bytes, keys, arbitrary paths or stream buffers cross Tauri IPC.
        Thread({
            try { socket.inputStream.copyTo(FileOutputStream(pair[1].fileDescriptor), 16 * 1024) }
            catch (_: Exception) { } finally { close() }
        }, "InvarDrop-RFCOMM-read").start()
        Thread({
            try { FileInputStream(pair[1].fileDescriptor).copyTo(socket.outputStream, 16 * 1024) }
            catch (_: Exception) { } finally { close() }
        }, "InvarDrop-RFCOMM-write").start()
    }
    @Command
    fun cancelConnect(invoke: Invoke) {
        val key = invoke.parseArgs(CancelBluetoothArgs::class.java).request
        connecting.remove(key)?.let { sockets.remove(it); try { it.close() } catch (_: Exception) { } }
        invoke.resolve()
    }
    @Command
    fun poll(invoke: Invoke) {
        if (enabled.get() && status != "unavailable") {
            try {
                if (!permitted()) reportFailure(BluetoothFailure("permission check", "permission was revoked"))
                else if (adapter?.isEnabled != true) reportFailure(BluetoothFailure("adapter power check", "enable Bluetooth in system settings"))
            } catch (failure: Exception) { reportFailure(BluetoothFailure("adapter and permission poll", failure.javaClass.simpleName)) }
        }
        if (status == "unavailable") {
            val failure = error
            shutdown()
            invoke.resolveObject(mapOf("status" to "unavailable", "error" to failure,
                "devices" to emptyList<Any>(), "incoming" to emptyList<Any>(), "failures" to emptyList<Any>()))
            return
        }
        val now = SystemClock.elapsedRealtime()
        remotes.entries.forEach { (id, remote) ->
            if (now - remote.observed >= BluetoothPolicy.PEER_MAX_AGE_MILLIS) remotes.remove(id, remote)
        }
        val streams = synchronized(incoming) {
            val result = mutableListOf<Map<String, Any>>()
            while (incoming.isNotEmpty()) {
                val item = incoming.removeFirst(); val handle = UUID.randomUUID().toString()
                synchronized(handoffs) { handoffs[handle] = item.descriptor }
                handler.postDelayed({ synchronized(handoffs) { handoffs.remove(handle)?.let { try { it.close() } catch (_: Exception) { } } } }, 10_000)
                result.add(mapOf("handle" to handle, "request" to item.request))
            }
            result
        }
        val failed = synchronized(failures) { failures.toList().also { failures.clear() } }
        invoke.resolveObject(mapOf("status" to status, "error" to error,
            "devices" to remotes.entries.map { mapOf("nativeId" to it.key, "info" to it.value.info, "sequence" to it.value.sequence) },
            "incoming" to streams, "failures" to failed))
    }
    @Command
    fun stop(invoke: Invoke) { shutdown(); invoke.resolve() }
    private fun shutdown() {
        enabled.set(false); signal = null; if (active === this) active = null; generation++; stopScan()
        stopAdvertising()
        try { server?.close() } catch (_: Exception) { }; server = null
        try { gattServer?.close() } catch (_: Exception) { }; gattServer = null
        gattRegistered = false
        adapter = null
        clients.toList().forEach { closeGatt(it) }
        sockets.toList().forEach { try { it.close() } catch (_: Exception) { } }; sockets.clear(); connecting.clear()
        synchronized(incoming) { while (incoming.isNotEmpty()) try { incoming.removeFirst().descriptor.close() } catch (_: Exception) { } }
        synchronized(handoffs) { handoffs.values.forEach { try { it.close() } catch (_: Exception) { } }; handoffs.clear() }
        remotes.clear(); readDevices.clear(); prepared.clear(); mtus.clear(); synchronized(reverseTokens) { reverseTokens.clear(); lastReverse = -10_000 }
        synchronized(failures) { failures.clear() }
        status = "disabled"; error = null
    }
    override fun onPause() {
        paused = true
        pausedAt = SystemClock.elapsedRealtime()
        stopScan()
        stopAdvertising()
        clients.toList().forEach { closeGatt(it) }
        updateStatus()
    }
    override fun onResume() {
        val refresh = paused && SystemClock.elapsedRealtime() - pausedAt >= BluetoothPolicy.SCAN_MILLIS
        paused = false
        if (enabled.get() && permitted()) {
            advertise()
            if (refresh) try { scan() } catch (failure: BluetoothFailure) { reportFailure(failure) }
        }
        updateStatus()
    }
    override fun onDestroy() { shutdown() }
}
