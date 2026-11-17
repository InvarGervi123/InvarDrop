package io.github.invargervi.invardrop

internal object NetworkPermissionPolicy {
    const val PERMISSION = "android.permission.ACCESS_LOCAL_NETWORK"
    // Do not request the Android 16 experimental permission or unrelated radios.
    fun required(sdk: Int, targetSdk: Int): Boolean = sdk >= 37 && targetSdk >= 37
}
