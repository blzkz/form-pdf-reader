package io.github.blzkz.formpdfreader

import android.content.Context
import android.net.Uri
import android.text.format.DateUtils
import android.text.format.Formatter
import org.json.JSONArray
import org.json.JSONObject

/**
 * A recent document: a content URI ([uri], from the system picker or another
 * app) or, for file:// links, a path on the device ([path]).
 */
data class DocItem(
    val name: String,
    val path: String? = null,
    val uri: String? = null,
    val size: Long = 0,
    /** Last modified or last opened, in milliseconds. */
    val time: Long = 0,
) {
    val key: String get() = path ?: uri.orEmpty()

    val contentUri: Uri? get() = uri?.let(Uri::parse)

    /** "1.2 MB · 3 days ago" */
    fun details(c: Context): String = listOfNotNull(
        size.takeIf { it > 0 }?.let { Formatter.formatShortFileSize(c, it) },
        time.takeIf { it > 0 }?.let {
            DateUtils.getRelativeTimeSpanString(it, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS)
        },
    ).joinToString(" · ")
}

/** Recently opened documents, newest first. */
object Recents {
    private const val MAX = 30
    private const val KEY = "items"

    private fun prefs(c: Context) = c.getSharedPreferences("recents", Context.MODE_PRIVATE)

    fun list(c: Context): List<DocItem> {
        val json = prefs(c).getString(KEY, null) ?: return emptyList()
        return runCatching {
            val a = JSONArray(json)
            (0 until a.length()).map { i ->
                val o = a.getJSONObject(i)
                DocItem(
                    name = o.getString("name"),
                    path = o.optString("path").ifEmpty { null },
                    uri = o.optString("uri").ifEmpty { null },
                    size = o.optLong("size"),
                    time = o.optLong("time"),
                )
            }
        }.getOrDefault(emptyList())
    }

    fun add(c: Context, item: DocItem) {
        save(c, listOf(item) + list(c).filter { it.key != item.key })
    }

    fun remove(c: Context, key: String) {
        save(c, list(c).filter { it.key != key })
    }

    private fun save(c: Context, items: List<DocItem>) {
        val a = JSONArray()
        for (it in items.take(MAX)) {
            a.put(
                JSONObject()
                    .put("name", it.name)
                    .put("path", it.path.orEmpty())
                    .put("uri", it.uri.orEmpty())
                    .put("size", it.size)
                    .put("time", it.time),
            )
        }
        prefs(c).edit().putString(KEY, a.toString()).apply()
    }
}
