package io.github.blzkz.formpdfreader

import android.content.Context
import org.json.JSONObject
import java.io.File

/**
 * Unsaved changes kept on the device, so they survive the app being closed
 * with them: by the system while it is in the background, by a crash, or by
 * swiping it away. Each document with changes has a copy of the PDF as it
 * would be saved, and where it has to be saved ([Draft.item]).
 *
 * The viewer writes the drafts when it goes to the background and deletes
 * each one when its document is saved or its changes are discarded. They are
 * in the app's private files (not in the cache, which Android may empty).
 */
object Drafts {
    class Draft(val id: String, val item: DocItem, val file: File, val time: Long)

    /** Drafts of the documents open in the viewer now. */
    val open: MutableSet<String> = java.util.Collections.synchronizedSet(HashSet())

    private fun dir(c: Context) = File(c.filesDir, "drafts").apply { mkdirs() }

    /** Saves the draft [id] of [item] (each file is replaced at once). */
    fun write(c: Context, id: String, item: DocItem, bytes: ByteArray) {
        val d = dir(c)
        replace(File(d, "$id.pdf"), bytes)
        val meta = JSONObject()
            .put("name", item.name)
            .put("path", item.path.orEmpty())
            .put("uri", item.uri.orEmpty())
            .put("time", System.currentTimeMillis())
        replace(File(d, "$id.json"), meta.toString().toByteArray())
    }

    private fun replace(f: File, bytes: ByteArray) {
        val tmp = File(f.parentFile, "${f.name}.tmp")
        tmp.outputStream().use {
            it.write(bytes)
            it.fd.sync()
        }
        if (!tmp.renameTo(f)) {
            tmp.delete()
            error("rename ${f.name}")
        }
    }

    fun delete(c: Context, id: String) {
        val d = dir(c)
        File(d, "$id.json").delete()
        File(d, "$id.pdf").delete()
    }

    /** Every draft, newest first (without the documents open in the viewer if [notOpen]). */
    fun list(c: Context, notOpen: Boolean = false): List<Draft> {
        val d = dir(c)
        return d.listFiles { f -> f.name.endsWith(".json") }.orEmpty().mapNotNull { f ->
            val id = f.name.removeSuffix(".json")
            val pdf = File(d, "$id.pdf")
            runCatching {
                val o = JSONObject(f.readText())
                val item = DocItem(
                    name = o.getString("name"),
                    path = o.optString("path").ifEmpty { null },
                    uri = o.optString("uri").ifEmpty { null },
                )
                Draft(id, item, pdf, o.optLong("time"))
            }.getOrNull()?.takeIf { pdf.exists() && !(notOpen && id in open) }
        }.sortedByDescending { it.time }
    }

    /** The draft of the document [key], if it is not open in the viewer. */
    fun find(c: Context, key: String): Draft? = list(c, notOpen = true).firstOrNull { it.item.key == key }
}
