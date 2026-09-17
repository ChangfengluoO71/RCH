package com.rch.reader

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Small Android Keystore-backed vault. The Keystore key never leaves the
 * hardware/OS boundary; only AES-GCM ciphertext is written to app-private
 * storage. Values are deliberately not placed in SharedPreferences so this
 * implementation does not depend on the deprecated EncryptedSharedPreferences.
 */
class CredentialVault(context: Context) {
    companion object {
        private const val KEY_ALIAS = "rch.credentials.v1"
        private const val FILE_NAME = "rch_credentials_v1.enc"
        private const val IV_BYTES = 12
        private const val TAG_BITS = 128
    }

    private val file = File(context.filesDir, FILE_NAME)
    @Synchronized
    fun put(key: String, value: String) {
        require(key.isNotBlank()) { "credential key must not be blank" }
        val values = readValues().apply { put(key, value) }
        writeValues(values)
    }

    @Synchronized
    fun get(key: String): String? {
        require(key.isNotBlank()) { "credential key must not be blank" }
        return readValues().optString(key, null)
    }

    @Synchronized
    fun delete(key: String) {
        require(key.isNotBlank()) { "credential key must not be blank" }
        val values = readValues()
        values.remove(key)
        if (values.length() == 0) {
            if (file.exists() && !file.delete()) error("unable to delete credential vault")
        } else {
            writeValues(values)
        }
    }

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val existing = store.getKey(KEY_ALIAS, null)
        if (existing is SecretKey) return existing
        val generator = KeyGenerator.getInstance(
            KeyProperties.KEY_ALGORITHM_AES,
            "AndroidKeyStore",
        )
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setKeySize(256)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true)
                .build(),
        )
        return generator.generateKey()
    }

    private fun readValues(): JSONObject {
        if (!file.exists()) return JSONObject()
        val payload = file.readBytes()
        require(payload.size > IV_BYTES) { "credential vault is truncated" }
        val iv = payload.copyOfRange(0, IV_BYTES)
        val ciphertext = payload.copyOfRange(IV_BYTES, payload.size)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, iv))
        return JSONObject(String(cipher.doFinal(ciphertext), Charsets.UTF_8))
    }

    private fun writeValues(values: JSONObject) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        // AndroidKeyStore rejects caller-provided IVs when randomized
        // encryption is required. Let the provider generate the IV, then
        // persist it alongside the ciphertext for the decrypt path.
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val iv = cipher.iv
        require(iv.size == IV_BYTES) { "unexpected credential vault IV size" }
        val ciphertext = cipher.doFinal(values.toString().toByteArray(Charsets.UTF_8))
        val temporary = File(file.parentFile, "${file.name}.part")
        FileOutputStream(temporary).use { stream ->
            stream.write(iv)
            stream.write(ciphertext)
            stream.flush()
            stream.fd.sync()
        }
        // Keep the previous ciphertext recoverable until the replacement has
        // been committed. File.renameTo is atomic on the same filesystem but
        // does not consistently replace an existing file across Android
        // providers, so use a rollback backup for the fallback path.
        val backup = File(file.parentFile, "${file.name}.bak")
        if (backup.exists() && !backup.delete()) {
            temporary.delete()
            error("unable to prepare credential vault backup")
        }
        val hadPrevious = file.exists()
        if (hadPrevious && !file.renameTo(backup)) {
            temporary.delete()
            error("unable to stage previous credential vault")
        }
        if (!temporary.renameTo(file)) {
            if (hadPrevious) {
                // A provider may leave a zero/partial destination behind even
                // when renameTo reports failure. Remove that candidate before
                // restoring the known-good ciphertext.
                if (file.exists()) file.delete()
                backup.renameTo(file)
            }
            temporary.delete()
            error("unable to commit credential vault")
        }
        if (hadPrevious && !backup.delete()) {
            // The new vault is valid; retain the backup rather than risking
            // data loss. It is excluded from Android backup as well.
            return
        }
    }
}
