package com.indicatrixsuite.mobile

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Exportable TLS keys are wrapped by an Android Keystore AES key at rest.
 * This does not claim the TLS key itself is hardware backed or non-exportable. */
internal class RemoteCredentialVault(context: Context) {
    private val file = AtomicFile(File(context.noBackupFilesDir, "remote-credentials-v1.bin"))
    private val alias = "indicatrix.remote.credentials.v1"

    /** Encrypts [bundle] under the Keystore key (created on first use) and replaces the file. */
    fun store(bundle: String) {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key(true))
        val iv = cipher.iv
        val sealed = cipher.doFinal(bundle.toByteArray(Charsets.UTF_8))
        val out = file.startWrite()
        try {
            out.write(iv.size)
            out.write(iv)
            out.write(sealed)
            file.finishWrite(out)
        } catch (e: Exception) {
            file.failWrite(out)
            throw e
        }
    }

    /** The stored bundle, or `null` when none is stored. Throws when it cannot be unlocked. */
    fun load(): String? {
        if (!file.baseFile.exists()) return null
        val data = file.readFully()
        require(data.size > 1) { "The stored remote credentials are damaged; enroll again." }
        val ivSize = data[0].toInt() and 0xFF
        require(data.size > 1 + ivSize) { "The stored remote credentials are damaged; enroll again." }
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(false), GCMParameterSpec(TAG_BITS, data, 1, ivSize))
        val plain = cipher.doFinal(data, 1 + ivSize, data.size - 1 - ivSize)
        return String(plain, Charsets.UTF_8)
    }

    /** Deletes the encrypted file and the Keystore key. */
    fun forget() {
        file.delete()
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        if (store.containsAlias(alias)) store.deleteEntry(alias)
    }

    private companion object {
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val TAG_BITS = 128
    }

    private fun key(create: Boolean): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        check(create) { "The remote credential wrapping key is unavailable; enroll again." }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true).build())
        }.generateKey()
    }
}
