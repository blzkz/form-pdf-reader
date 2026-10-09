package io.github.blzkz.formpdfreader

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Color
import android.graphics.pdf.PdfRenderer
import android.os.ParcelFileDescriptor
import android.util.LruCache
import android.widget.ImageView
import java.io.File
import kotlin.math.roundToInt

/**
 * First-page thumbnails, rendered with Android's own PDF renderer: fast and
 * independent of PDFium. Dynamic XFA forms show their placeholder page
 * ("please wait…"), which is what they contain without an XFA engine.
 */
object Thumbnails {
    private const val WIDTH = 240

    private val cache = object : LruCache<String, Bitmap>(16 * 1024 * 1024) {
        override fun sizeOf(key: String, value: Bitmap) = value.byteCount
    }

    /** Documents that could not be rendered (UI thread only). */
    private val failed = HashSet<String>()

    fun load(view: ImageView, item: DocItem) {
        val key = item.key
        view.tag = key
        cache.get(key)?.let { return show(view, it) }
        view.scaleType = ImageView.ScaleType.CENTER
        view.setImageResource(R.drawable.ic_doc)
        if (key in failed) return
        val c = view.context.applicationContext
        Io.run({ render(c, item) }) { r ->
            val bmp = r.getOrNull()
            if (bmp == null) {
                failed += key
                return@run
            }
            cache.put(key, bmp)
            if (view.tag == key) show(view, bmp)
        }
    }

    /** Drops the thumbnail of a document that has just been saved. */
    fun forget(key: String) {
        cache.remove(key)
        failed -= key
    }

    private fun show(view: ImageView, bmp: Bitmap) {
        view.scaleType = ImageView.ScaleType.FIT_CENTER
        view.setImageBitmap(bmp)
    }

    private fun render(c: Context, item: DocItem): Bitmap {
        val fd = if (item.path != null) {
            ParcelFileDescriptor.open(File(item.path), ParcelFileDescriptor.MODE_READ_ONLY)
        } else {
            requireNotNull(c.contentResolver.openFileDescriptor(requireNotNull(item.contentUri), "r"))
        }
        // The renderer owns the descriptor and closes it.
        val renderer = try {
            PdfRenderer(fd)
        } catch (e: Exception) {
            fd.close()
            throw e
        }
        try {
            val page = renderer.openPage(0)
            try {
                val h = (WIDTH * page.height.toFloat() / page.width).roundToInt().coerceIn(1, WIDTH * 2)
                val bmp = Bitmap.createBitmap(WIDTH, h, Bitmap.Config.ARGB_8888)
                bmp.eraseColor(Color.WHITE)
                page.render(bmp, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                return bmp
            } finally {
                page.close()
            }
        } finally {
            renderer.close()
        }
    }
}
