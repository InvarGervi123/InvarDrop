package io.github.invargervi.invardrop

fun main() {
    check(!NetworkPermissionPolicy.required(36, 37))
    check(!NetworkPermissionPolicy.required(37, 36))
    check(NetworkPermissionPolicy.required(37, 37))
    check(NetworkPermissionPolicy.required(38, 37))
    println("PASS: pre-Android-17 devices need no new permission")
    println("PASS: legacy target SDKs do not request ACCESS_LOCAL_NETWORK")
    println("PASS: SDK/target 37 require the platform permission")
    println("PASS: newer platforms keep the permission gate")
    println("4 Android network permission policy checks passed")
}
