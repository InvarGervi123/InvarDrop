//! JNI bridge to platform AndroidKeyStore and AES/GCM/NoPadding. No Kotlin/JS
//! key API, exportable wrapping key, password, or alternate provider is used.
use super::{IV_LEN, ProtectedKey};
use crate::secure::error::{Error, Result};
use jni::{
    JNIEnv, JavaVM,
    objects::{JByteArray, JObject, JValue},
};
use zeroize::Zeroizing;

const PROVIDER: &str = "AndroidKeyStore";
const TRANSFORMATION: &str = "AES/GCM/NoPadding";
pub(super) struct Keystore {
    alias: String,
}
impl Keystore {
    pub(super) fn new(account: &str) -> Self {
        Self {
            alias: format!("io.github.invargervi.invardrop.master-v1.{account}"),
        }
    }
}

fn unavailable(stage: &str) -> Error {
    // Fixed operation names only: Java exceptions can contain sensitive data.
    eprintln!("Android secure storage unavailable: {stage}");
    Error::Storage
}
fn checked<T>(result: jni::errors::Result<T>, stage: &str) -> Result<T> {
    result.map_err(|_| unavailable(stage))
}
fn with_env<T>(action: impl FnOnce(&mut JNIEnv<'_>) -> Result<T>) -> Result<T> {
    // Tauri/Tao initializes ndk-context with a process-lifetime application
    // GlobalRef before setup. Refuse use before that point instead of panicking.
    if tauri::tao::platform::android::prelude::main_android_context().is_none() {
        return Err(unavailable("application context not initialized"));
    }
    let context = ndk_context::android_context();
    // SAFETY: the VM pointer belongs to the running Tauri Android application.
    // JavaVM::from_raw does not take ownership or destroy the platform VM.
    let vm = checked(
        unsafe { JavaVM::from_raw(context.vm().cast()) },
        "JVM access",
    )?;
    let mut env = checked(vm.attach_current_thread(), "JVM thread attachment")?;
    let result = action(&mut env);
    if env.exception_check().unwrap_or(true) {
        let _ = env.exception_clear();
        return Err(unavailable("JNI exception"));
    }
    result
}
fn key_store<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>> {
    let provider = checked(env.new_string(PROVIDER), "provider name")?;
    let store = checked(
        env.call_static_method(
            "java/security/KeyStore",
            "getInstance",
            "(Ljava/lang/String;)Ljava/security/KeyStore;",
            &[(&provider).into()],
        ),
        "Keystore provider",
    )?;
    let store = checked(store.l(), "Keystore object")?;
    checked(
        env.call_method(
            &store,
            "load",
            "(Ljava/security/KeyStore$LoadStoreParameter;)V",
            &[JValue::Object(&JObject::null())],
        ),
        "Keystore load",
    )?;
    Ok(store)
}
fn contains(env: &mut JNIEnv<'_>, store: &JObject<'_>, alias: &str) -> Result<bool> {
    let alias = checked(env.new_string(alias), "key alias")?;
    let value = checked(
        env.call_method(
            store,
            "containsAlias",
            "(Ljava/lang/String;)Z",
            &[(&alias).into()],
        ),
        "key lookup",
    )?;
    checked(value.z(), "key lookup result")
}
fn get_key<'a>(env: &mut JNIEnv<'a>, alias: &str) -> Result<JObject<'a>> {
    let store = key_store(env)?;
    let alias = checked(env.new_string(alias), "key alias")?;
    let result = checked(
        env.call_method(
            &store,
            "getKey",
            "(Ljava/lang/String;[C)Ljava/security/Key;",
            &[(&alias).into(), JValue::Object(&JObject::null())],
        ),
        "protected key access",
    )?;
    let key = checked(result.l(), "protected key result")?;
    if key.is_null() {
        return Err(Error::Corrupt);
    }
    Ok(key)
}
fn cipher<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>> {
    let transformation = checked(env.new_string(TRANSFORMATION), "cipher name")?;
    let result = checked(
        env.call_static_method(
            "javax/crypto/Cipher",
            "getInstance",
            "(Ljava/lang/String;)Ljavax/crypto/Cipher;",
            &[(&transformation).into()],
        ),
        "AES-GCM cipher",
    )?;
    checked(result.l(), "cipher result")
}
fn aad(env: &mut JNIEnv<'_>, cipher: &JObject<'_>, bytes: &[u8]) -> Result<()> {
    let bytes = checked(env.byte_array_from_slice(bytes), "AAD buffer")?;
    checked(
        env.call_method(cipher, "updateAAD", "([B)V", &[(&bytes).into()]),
        "AAD binding",
    )?;
    Ok(())
}

impl ProtectedKey for Keystore {
    fn exists(&self) -> Result<bool> {
        with_env(|env| {
            let store = key_store(env)?;
            contains(env, &store, &self.alias)
        })
    }
    fn create(&self) -> Result<()> {
        with_env(|env| {
            let store = key_store(env)?;
            if contains(env, &store, &self.alias)? {
                return Err(Error::Corrupt);
            }
            let alias = checked(env.new_string(&self.alias), "key alias")?;
            let spec = checked(
                env.new_object(
                    "android/security/keystore/KeyGenParameterSpec$Builder",
                    "(Ljava/lang/String;I)V",
                    &[(&alias).into(), 3.into()],
                ),
                "key specification",
            )?;
            checked(
                env.call_method(
                    &spec,
                    "setKeySize",
                    "(I)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
                    &[256.into()],
                ),
                "AES-256 key size",
            )?;
            for (method, value) in [
                ("setBlockModes", "GCM"),
                ("setEncryptionPaddings", "NoPadding"),
            ] {
                let value = checked(env.new_string(value), "key restriction")?;
                let array = checked(
                    env.new_object_array(1, "java/lang/String", &value),
                    "key restriction array",
                )?;
                checked(env.call_method(&spec, method, "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;", &[(&array).into()]), "key restrictions")?;
            }
            checked(
                env.call_method(
                    &spec,
                    "setRandomizedEncryptionRequired",
                    "(Z)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
                    &[true.into()],
                ),
                "random nonce requirement",
            )?;
            let spec = checked(
                env.call_method(
                    &spec,
                    "build",
                    "()Landroid/security/keystore/KeyGenParameterSpec;",
                    &[],
                ),
                "key specification build",
            )?;
            let spec = checked(spec.l(), "key specification result")?;
            let algorithm = checked(env.new_string("AES"), "key algorithm")?;
            let provider = checked(env.new_string(PROVIDER), "provider name")?;
            let generator = checked(
                env.call_static_method(
                    "javax/crypto/KeyGenerator",
                    "getInstance",
                    "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
                    &[(&algorithm).into(), (&provider).into()],
                ),
                "Keystore key generator",
            )?;
            let generator = checked(generator.l(), "key generator result")?;
            checked(
                env.call_method(
                    &generator,
                    "init",
                    "(Ljava/security/spec/AlgorithmParameterSpec;)V",
                    &[(&spec).into()],
                ),
                "key generator initialization",
            )?;
            checked(
                env.call_method(&generator, "generateKey", "()Ljavax/crypto/SecretKey;", &[]),
                "non-exportable key generation",
            )?;
            Ok(())
        })
    }
    fn encrypt(&self, master: &[u8], authenticated: &[u8]) -> Result<Vec<u8>> {
        if master.len() != 32 {
            return Err(Error::Storage);
        }
        with_env(|env| {
            let key = get_key(env, &self.alias)?;
            let cipher = cipher(env)?;
            checked(
                env.call_method(
                    &cipher,
                    "init",
                    "(ILjava/security/Key;)V",
                    &[1.into(), (&key).into()],
                ),
                "encryption initialization",
            )?;
            aad(env, &cipher, authenticated)?;
            let input = checked(env.byte_array_from_slice(master), "master buffer")?;
            let result = env.call_method(&cipher, "doFinal", "([B)[B", &[(&input).into()]);
            if result.is_err() {
                let _ = env.exception_clear();
            }
            let _ = env.set_byte_array_region(&input, 0, &[0; 32]);
            let ciphertext = checked(
                checked(result, "master encryption")?.l(),
                "ciphertext result",
            )?;
            let ciphertext = checked(
                env.convert_byte_array(JByteArray::from(ciphertext)),
                "ciphertext bytes",
            )?;
            let iv = checked(env.call_method(&cipher, "getIV", "()[B", &[]), "random IV")?;
            let iv = checked(iv.l(), "IV result")?;
            let iv = checked(env.convert_byte_array(JByteArray::from(iv)), "IV bytes")?;
            if iv.len() != IV_LEN || ciphertext.len() != 48 {
                return Err(Error::Storage);
            }
            Ok([iv, ciphertext].concat())
        })
    }
    fn decrypt(&self, sealed: &[u8], authenticated: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if sealed.len() != IV_LEN + 48 {
            return Err(Error::Corrupt);
        }
        with_env(|env| {
            let key = get_key(env, &self.alias)?;
            let cipher = cipher(env)?;
            let iv = checked(env.byte_array_from_slice(&sealed[..IV_LEN]), "IV buffer")?;
            let spec = checked(
                env.new_object(
                    "javax/crypto/spec/GCMParameterSpec",
                    "(I[B)V",
                    &[128.into(), (&iv).into()],
                ),
                "GCM tag specification",
            )?;
            checked(
                env.call_method(
                    &cipher,
                    "init",
                    "(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
                    &[2.into(), (&key).into(), (&spec).into()],
                ),
                "decryption initialization",
            )?;
            aad(env, &cipher, authenticated)?;
            let input = checked(
                env.byte_array_from_slice(&sealed[IV_LEN..]),
                "ciphertext buffer",
            )?;
            let result = env.call_method(&cipher, "doFinal", "([B)[B", &[(&input).into()]);
            let result = match result {
                Ok(value) => value,
                Err(_) => {
                    let exception = env.exception_occurred().ok();
                    let _ = env.exception_clear();
                    let corrupt = exception.is_some_and(|e| {
                        env.is_instance_of(e, "javax/crypto/AEADBadTagException")
                            .unwrap_or(false)
                    });
                    return Err(if corrupt {
                        Error::Corrupt
                    } else {
                        unavailable("master decryption")
                    });
                }
            };
            let output = JByteArray::from(checked(result.l(), "decrypted result")?);
            let master = Zeroizing::new(checked(
                env.convert_byte_array(&output),
                "decrypted master buffer",
            )?);
            if master.len() == 32 {
                let _ = env.set_byte_array_region(&output, 0, &[0; 32]);
            }
            Ok(master)
        })
    }
}
