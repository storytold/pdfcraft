package io.github.dikshant363.pdfcraft.engine

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.dikshant363.pdfcraft.core.DigitalIdInfo
import java.io.File
import org.json.JSONArray
import org.json.JSONObject

/** A digital ID the app keeps for signing: its file and what its certificate says. Times are seconds since 1970. */
data class DigitalId(
    val file: File,
    val name: String,
    val issuer: String,
    val email: String?,
    val validFrom: Long,
    val validTo: Long,
    val selfSigned: Boolean,
    val sha256: String,
) {
    fun expired(nowMs: Long = System.currentTimeMillis()) = nowMs / 1000 > validTo
    fun notYetValid(nowMs: Long = System.currentTimeMillis()) = nowMs / 1000 < validFrom
}

/**
 * The digital IDs (.p12 / .pfx files) the reader added or made, copied into the app's private
 * files with a small JSON list of their certificates. Their passwords are never kept: signing
 * asks for one each time.
 */
class DigitalIds(dir: File) {
    private val root = File(dir, "digital-ids").apply { mkdirs() }
    private val index = File(root, "ids.json")
    private val lock = Any()

    var all by mutableStateOf(load())
        private set

    /** Keep a copy of the ID file [source] (its certificate described by [info]); one with the same certificate is replaced. */
    fun add(source: File, info: DigitalIdInfo): DigitalId = synchronized(lock) {
        val target = File(root, fileKey(info.sha256) + ".p12")
        if (source.canonicalPath != target.canonicalPath) source.copyTo(target, overwrite = true)
        val id = DigitalId(target, info.name, info.issuer, info.email, info.validFrom, info.validTo, info.selfSigned, info.sha256)
        all = all.filter { it.sha256 != info.sha256 } + id
        save()
        id
    }

    fun remove(id: DigitalId) = synchronized(lock) {
        id.file.delete()
        all = all.filter { it.sha256 != id.sha256 }
        save()
    }

    private fun load(): List<DigitalId> = runCatching {
        if (!index.isFile) return@runCatching emptyList()
        val a = JSONArray(index.readText())
        (0 until a.length()).mapNotNull { i ->
            val o = a.optJSONObject(i) ?: return@mapNotNull null
            val file = File(root, o.optString("file"))
            if (!file.isFile) return@mapNotNull null
            DigitalId(
                file, o.optString("name"), o.optString("issuer"), o.optString("email").ifEmpty { null },
                o.optLong("validFrom"), o.optLong("validTo"), o.optBoolean("selfSigned"), o.optString("sha256"),
            )
        }
    }.getOrDefault(emptyList())

    private fun save() {
        val a = JSONArray()
        for (id in all) {
            a.put(
                JSONObject().put("file", id.file.name).put("name", id.name).put("issuer", id.issuer).put("email", id.email ?: "")
                    .put("validFrom", id.validFrom).put("validTo", id.validTo).put("selfSigned", id.selfSigned).put("sha256", id.sha256),
            )
        }
        writeAtomically(index, a.toString())
    }
}

/**
 * The signers' certificates the reader chose to trust (DER files in the app's private files): the
 * engine is given them at start, and a signature by one of them validates as trusted.
 */
class TrustedCertificates(dir: File) {
    private val root = File(dir, "trusted").apply { mkdirs() }

    /** Bumped on every change, for what shows whether a signer is trusted. */
    var version by mutableStateOf(0)
        private set

    fun files(): List<File> = root.listFiles { f -> f.isFile && f.extension == "cer" }.orEmpty().sortedBy { it.name }

    fun contains(sha256: String) = File(root, fileKey(sha256) + ".cer").isFile

    fun add(der: ByteArray, sha256: String) {
        File(root, fileKey(sha256) + ".cer").writeBytes(der)
        version++
    }

    fun remove(sha256: String) {
        File(root, fileKey(sha256) + ".cer").delete()
        version++
    }
}

/** A certificate fingerprint (hex, any spacing) as a file name. */
private fun fileKey(sha256: String) = sha256.filter { it.isLetterOrDigit() }.lowercase().ifEmpty { "id" }

private fun writeAtomically(file: File, text: String) {
    runCatching {
        val tmp = File(file.parentFile, file.name + ".tmp")
        tmp.writeText(text)
        if (!tmp.renameTo(file)) {
            tmp.copyTo(file, overwrite = true)
            tmp.delete()
        }
    }.onFailure {
        android.util.Log.w("PdfCraft", "failed to write ${file.name}: ${it.message}")
    }
}
