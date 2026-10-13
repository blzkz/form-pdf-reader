package io.github.blzkz.formpdfreader

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import android.graphics.pdf.PdfRenderer
import android.os.ParcelFileDescriptor
import android.util.LruCache
import android.widget.ImageView
import java.io.File
import java.nio.ByteBuffer
import java.security.MessageDigest
import kotlin.math.min
import kotlin.math.roundToInt
import uniffi.form_pdf_reader_ffi.PdfDocument

/**
 * First-page thumbnails of the recent documents.
 *
 * When a document is opened or saved in the viewer, its first page is drawn
 * with the app's own engine and kept on the device ([capture]): it shows the
 * data filled in, and dynamic XFA forms look as they do in the viewer (other
 * renderers only see their placeholder page, "please wait…" or "this viewer
 * is not supported"). Documents never opened in the viewer use Android's own
 * PDF renderer, which is fast and independent of PDFium.
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
        Io.run({ saved(c, key) ?: render(c, item) }) { r ->
            val bmp = r.getOrNull()
            if (bmp == null) {
                failed += key
                return@run
            }
            cache.put(key, bmp)
            if (view.tag == key) show(view, bmp)
        }
    }

    /**
     * Draws the first page of [doc], open in the viewer, as the thumbnail of
     * [key] and keeps it on the device. Call from the UI thread.
     */
    fun capture(c: Context, key: String, doc: PdfDocument) {
        val app = c.applicationContext
        PdfEngine.run({
            val size = doc.pageSize(0u)
            val w = WIDTH
            val pageH = (w * size.height / size.width).roundToInt().coerceAtLeast(1)
            // A dynamic XFA form is one very long page: its top, A4-shaped.
            val content = (w * doc.contentHeight(0u) / size.width).roundToInt()
            val h = min(min(pageH, content), if (doc.isContinuous()) (w * 1.42f).roundToInt() else w * 2).coerceAtLeast(1)
            Triple(doc.render(0u, w, pageH, 0, 0, w, h), w, h)
        }) { r ->
            val (px, w, h) = r.getOrNull() ?: return@run
            if (px.size != w * h * 4) return@run
            val bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
            bmp.copyPixelsFromBuffer(ByteBuffer.wrap(px))
            cache.put(key, bmp)
            failed -= key
            Io.run({
                val f = fileFor(app, key)
                val tmp = File(f.parentFile, "${f.name}.tmp")
                tmp.outputStream().use { bmp.compress(Bitmap.CompressFormat.PNG, 100, it) }
                if (!tmp.renameTo(f)) tmp.delete()
            }) {}
        }
    }

    /** Deletes the kept thumbnails of documents that are no longer in [keys]. */
    fun prune(c: Context, keys: Collection<String>) {
        val keep = keys.map { "${hash(it)}.png" }.toSet()
        dir(c).listFiles()?.filter { it.name !in keep && !it.name.endsWith(".tmp") }?.forEach { it.delete() }
    }

    private fun dir(c: Context) = File(c.filesDir, "thumbs").apply { mkdirs() }

    private fun fileFor(c: Context, key: String) = File(dir(c), "${hash(key)}.png")

    private fun hash(key: String): String =
        MessageDigest.getInstance("SHA-1").digest(key.toByteArray()).joinToString("") { "%02x".format(it) }

    private fun saved(c: Context, key: String): Bitmap? =
        fileFor(c, key).takeIf { it.exists() }?.let { BitmapFactory.decodeFile(it.path) }

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
