package io.github.blzkz.formpdfreader

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import android.graphics.Paint
import android.media.ExifInterface
import android.net.Uri
import android.os.Bundle
import android.view.View
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.TextView
import android.widget.Toast
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.FileProvider
import androidx.core.view.WindowCompat
import androidx.core.view.isVisible
import com.google.android.material.appbar.MaterialToolbar
import com.google.android.material.button.MaterialButton
import com.google.android.material.button.MaterialButtonToggleGroup
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import java.io.File
import java.nio.ByteBuffer
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import uniffi.form_pdf_reader_ffi.OcrProgress
import uniffi.form_pdf_reader_ffi.OrientationModel
import uniffi.form_pdf_reader_ffi.ScanFilter
import uniffi.form_pdf_reader_ffi.ScanPage
import uniffi.form_pdf_reader_ffi.scanDetect
import uniffi.form_pdf_reader_ffi.scanFilter
import uniffi.form_pdf_reader_ffi.scanPdf

/**
 * Scanning paper documents: photos taken with the device's camera app (the
 * app needs no camera permission) or chosen from the gallery. For each
 * page the sheet is found in the photo (its corners can be moved), the
 * perspective straightened and a filter applied (colour, grey, or black
 * and white like a photocopy). The result is a PDF in the cache, returned
 * in [EXTRA_PATH].
 *
 * Everything happens on the device, with the app's own code.
 */
class ScanActivity : AppCompatActivity() {
    /** A page: its photo, the sheet's corners in it and how it is shown. */
    private class Page(val photo: File, var corners: FloatArray, var quarter: Int = 0, var filter: ScanFilter) {
        /** Turned by hand: the model does not turn it. */
        var manual = false
        /** The finished page (JPEG) and its size, once cropped. */
        var output: File? = null
        var width = 0
        var height = 0
    }

    private lateinit var toolbar: MaterialToolbar
    private lateinit var area: FrameLayout
    private lateinit var crop: CropView
    private lateinit var preview: ImageView
    private lateinit var empty: TextView
    private lateinit var progress: View
    private lateinit var hint: TextView
    private lateinit var filters: MaterialButtonToggleGroup
    private lateinit var rotate: MaterialButton
    private lateinit var retake: MaterialButton
    private lateinit var delete: MaterialButton
    private lateinit var add: MaterialButton
    private lateinit var next: MaterialButton
    private lateinit var ocr: com.google.android.material.checkbox.MaterialCheckBox

    private val pages = mutableListOf<Page>()
    private var current = -1
    /** Cropping the current page (otherwise its result is shown). */
    private var cropping = true
    /** The current page straightened, before the filter. */
    private var straight: Bitmap? = null
    private var pendingPhoto: File? = null
    private var replacing = -1
    private var busy = false

    private val dir by lazy { File(cacheDir, "scan").apply { mkdirs() } }

    private val takePicture = registerForActivityResult(ActivityResultContracts.TakePicture()) { ok ->
        val f = pendingPhoto
        pendingPhoto = null
        if (ok && f != null && f.length() > 0) addPhoto(f) else f?.delete()
        updateUi()
    }

    private val pickImage = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@registerForActivityResult updateUi()
        Io.run({ copy(uri) }) { r -> r.onSuccess { addPhoto(it) }.onFailure { e -> fail(e) } }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        PdfEngine.start(applicationContext)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        setContentView(R.layout.activity_scan)
        applySystemInsets(findViewById(R.id.root))
        toolbar = findViewById(R.id.toolbar)
        area = findViewById(R.id.scan_area)
        preview = findViewById(R.id.scan_preview)
        empty = findViewById(R.id.scan_empty)
        progress = findViewById(R.id.scan_progress)
        hint = findViewById(R.id.scan_hint)
        filters = findViewById(R.id.scan_filters)
        rotate = findViewById(R.id.scan_rotate)
        retake = findViewById(R.id.scan_retake)
        delete = findViewById(R.id.scan_delete)
        add = findViewById(R.id.scan_add)
        next = findViewById(R.id.scan_next)
        ocr = findViewById(R.id.scan_ocr)
        ocr.isChecked = Prefs.scanOcr(this)
        ocr.setOnCheckedChangeListener { _, on -> Prefs.setScanOcr(this, on) }
        crop = CropView(this)
        area.addView(crop, 0, FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT))

        toolbar.setNavigationOnClickListener { close() }
        toolbar.inflateMenu(R.menu.scan)
        toolbar.setOnMenuItemClickListener { item ->
            when (item.itemId) {
                R.id.scan_camera -> camera()
                R.id.scan_gallery -> gallery()
                R.id.scan_previous -> show(current - 1)
                R.id.scan_following -> show(current + 1)
            }
            true
        }
        onBackPressedDispatcher.addCallback(this, object : androidx.activity.OnBackPressedCallback(true) {
            override fun handleOnBackPressed() = close()
        })
        rotate.setOnClickListener { rotatePhoto() }
        retake.setOnClickListener {
            if (cropping) {
                replacing = current
                camera()
            } else {
                cropping = true
                show(current)
            }
        }
        delete.setOnClickListener { deletePage() }
        add.setOnClickListener { camera() }
        next.setOnClickListener { if (cropping) cropCurrent() else finishPdf() }
        filters.addOnButtonCheckedListener { _, id, checked ->
            if (!checked || cropping) return@addOnButtonCheckedListener
            val p = pages.getOrNull(current) ?: return@addOnButtonCheckedListener
            val f = when (id) {
                R.id.filter_gray -> ScanFilter.GRAY
                R.id.filter_bw -> ScanFilter.BLACK_WHITE
                else -> ScanFilter.COLOR
            }
            if (f != p.filter) {
                p.filter = f
                Prefs.setScanFilter(this, f.name)
                applyFilter(p)
            }
        }

        if (savedInstanceState != null) restore(savedInstanceState)
        updateUi()
        if (savedInstanceState == null) {
            val test = intent.getStringExtra(EXTRA_TEST_IMAGE)?.takeIf { applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE != 0 }
            when {
                // Test builds: a photo in the cache instead of the camera.
                test != null -> Io.run({ File(test).copyTo(File(dir, "foto-${System.nanoTime()}.jpg")) }) { r -> r.onSuccess { addPhoto(it) }.onFailure { e -> fail(e) } }
                intent.getBooleanExtra(EXTRA_GALLERY, false) -> gallery()
                else -> camera()
            }
        }
    }

    // -----------------------------------------------------------------------
    // Photos
    // -----------------------------------------------------------------------

    private fun camera() {
        val f = File(dir, "foto-${System.nanoTime()}.jpg")
        pendingPhoto = f
        val uri = FileProvider.getUriForFile(this, "$packageName.files", f)
        try {
            takePicture.launch(uri)
        } catch (_: ActivityNotFoundException) {
            pendingPhoto = null
            Toast.makeText(this, R.string.scan_no_camera, Toast.LENGTH_LONG).show()
            gallery()
        }
    }

    private fun gallery() = pickImage.launch(arrayOf("image/*"))

    private fun copy(uri: Uri): File {
        val f = File(dir, "foto-${System.nanoTime()}.jpg")
        requireNotNull(contentResolver.openInputStream(uri)) { uri.toString() }.use { i -> f.outputStream().use { i.copyTo(it) } }
        return f
    }

    /** The photo of [p] upright (EXIF and the turns given) and at most [maxSide] pixels. */
    private fun loadPhoto(p: Page, maxSide: Int): Bitmap {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(p.photo.absolutePath, bounds)
        var sample = 1
        while (max(bounds.outWidth, bounds.outHeight) / (sample * 2) >= maxSide) sample *= 2
        val b = requireNotNull(BitmapFactory.decodeFile(p.photo.absolutePath, BitmapFactory.Options().apply { inSampleSize = sample })) { p.photo.name }
        val exif = when (ExifInterface(p.photo.absolutePath).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL)) {
            ExifInterface.ORIENTATION_ROTATE_90 -> 90
            ExifInterface.ORIENTATION_ROTATE_180 -> 180
            ExifInterface.ORIENTATION_ROTATE_270 -> 270
            else -> 0
        }
        val deg = (exif + 90 * p.quarter) % 360
        val scale = min(1f, maxSide.toFloat() / max(b.width, b.height))
        if (deg == 0 && scale == 1f) return b
        val m = Matrix().apply {
            postScale(scale, scale)
            postRotate(deg.toFloat())
        }
        return Bitmap.createBitmap(b, 0, 0, b.width, b.height, m, true)
    }

    private fun addPhoto(photo: File) {
        val filter = runCatching { ScanFilter.valueOf(Prefs.scanFilter(this)) }.getOrDefault(ScanFilter.COLOR)
        val page = Page(photo, DEFAULT_CORNERS.copyOf(), filter = filter)
        val at = replacing.takeIf { it in pages.indices }
        replacing = -1
        if (at != null) {
            pages[at].photo.delete()
            pages[at].output?.delete()
            pages[at] = page
        } else {
            pages.add(page)
        }
        val index = at ?: (pages.size - 1)
        cropping = true
        working(true)
        // The sheet, found in a small grey copy of the photo.
        Io.run({
            val small = loadPhoto(page, DETECT_SIDE)
            val px = IntArray(small.width * small.height)
            small.getPixels(px, 0, small.width, 0, 0, small.width, small.height)
            val gray = ByteArray(px.size) { i ->
                val c = px[i]
                ((Color.red(c) * 299 + Color.green(c) * 587 + Color.blue(c) * 114) / 1000).toByte()
            }
            Triple(gray, small.width, small.height)
        }) { r ->
            val (gray, w, h) = r.getOrElse { e ->
                working(false)
                return@run fail(e)
            }
            PdfEngine.run({ scanDetect(gray, w.toUInt(), h.toUInt()) }) { d ->
                d.getOrNull()?.let { page.corners = it.toFloatArray() }
                working(false)
                show(index)
            }
        }
    }

    /** Turns the current photo 90° to the right (and the corners with it). */
    private fun rotatePhoto() {
        val p = pages.getOrNull(current) ?: return
        p.quarter = (p.quarter + 1) % 4
        p.manual = true
        val c = p.corners
        // (x, y) → (1 − y, x); the corners keep their order from the top left.
        val r = FloatArray(8) { i -> if (i % 2 == 0) 1f - c[i + 1] else c[i - 1] }
        p.corners = floatArrayOf(r[6], r[7], r[0], r[1], r[2], r[3], r[4], r[5])
        p.output?.delete()
        p.output = null
        show(current)
    }

    // -----------------------------------------------------------------------
    // Cropping and filters
    // -----------------------------------------------------------------------

    private fun show(index: Int) {
        if (index !in pages.indices) return
        if (index != current) {
            current = index
            straight = null
            cropping = pages[index].output == null
        }
        val p = pages[index]
        if (cropping) {
            working(true)
            Io.run({ loadPhoto(p, PHOTO_SIDE) }) { r ->
                working(false)
                r.onSuccess {
                    crop.bitmap = it
                    crop.corners = p.corners
                }.onFailure { e -> fail(e) }
                updateUi()
            }
        } else {
            p.output?.let { preview.setImageBitmap(BitmapFactory.decodeFile(it.absolutePath)) }
        }
        updateUi()
    }

    /** Straightens the sheet of the current page and applies its filter. */
    private fun cropCurrent() {
        val p = pages.getOrNull(current) ?: return
        p.corners = crop.corners.copyOf()
        val photo = crop.bitmap ?: return
        working(true)
        val auto = !p.manual
        Io.run({ straighten(photo, p.corners).let { if (auto) upright(applicationContext, it) else it } }) { r ->
            r.onSuccess {
                straight = it
                cropping = false
                applyFilter(p)
            }.onFailure { e ->
                working(false)
                fail(e)
            }
        }
    }

    private fun applyFilter(p: Page) {
        val src = straight ?: return show(current)
        val filter = p.filter
        working(true)
        val w = src.width
        val h = src.height
        val rgba = ByteBuffer.allocate(w * h * 4).also { src.copyPixelsToBuffer(it) }.array()
        PdfEngine.run({ scanFilter(rgba, w.toUInt(), h.toUInt(), filter) }) { r ->
            val px = r.getOrElse { e ->
                working(false)
                return@run fail(e)
            }
            Io.run({
                val out = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
                out.copyPixelsFromBuffer(ByteBuffer.wrap(px))
                val f = File(dir, "pagina-${System.nanoTime()}.jpg")
                f.outputStream().use { out.compress(Bitmap.CompressFormat.JPEG, JPEG_QUALITY, it) }
                Pair(out, f)
            }) { s ->
                working(false)
                val (bmp, f) = s.getOrElse { e -> return@run fail(e) }
                p.output?.delete()
                p.output = f
                p.width = w
                p.height = h
                if (pages.getOrNull(current) === p) preview.setImageBitmap(bmp)
                updateUi()
            }
        }
    }

    private fun deletePage() {
        val p = pages.getOrNull(current) ?: return
        p.photo.delete()
        p.output?.delete()
        pages.removeAt(current)
        straight = null
        val n = current.coerceAtMost(pages.size - 1)
        current = -1
        if (n >= 0) show(n) else updateUi()
    }

    // -----------------------------------------------------------------------
    // The PDF
    // -----------------------------------------------------------------------

    private fun finishPdf() {
        val done = pages.filter { it.output != null }
        if (done.isEmpty()) return
        working(true)
        Io.run({ done.map { ScanPage(it.output!!.readBytes(), it.width.toUInt(), it.height.toUInt()) } }) { r ->
            val list = r.getOrElse { e ->
                working(false)
                return@run fail(e)
            }
            val read = ocr.isChecked
            val onProgress = object : OcrProgress {
                override fun progress(done: UInt, total: UInt) {
                    runOnUiThread { if (done < total) hint.text = getString(R.string.ocr_progress, done.toInt() + 1, total.toInt()) }
                }
            }
            if (read) hint.setText(R.string.ocr_working)
            PdfEngine.run({
                val pdf = scanPdf(list)
                // The text of the pages as an invisible layer (if the models load).
                val engine = if (read) OcrModels.engine(applicationContext) else null
                engine?.addTextLayer(pdf, null, onProgress)?.pdf ?: pdf
            }) { pdf ->
                val bytes = pdf.getOrElse { e ->
                    working(false)
                    return@run fail(e)
                }
                Io.run({
                    val stamp = SimpleDateFormat("yyyy-MM-dd HH.mm.ss", Locale.ROOT).format(Date())
                    val out = File(File(cacheDir, "scans").apply { mkdirs() }, getString(R.string.scan_name, stamp) + ".pdf")
                    out.writeBytes(bytes)
                    out
                }) { w ->
                    working(false)
                    val out = w.getOrElse { e -> return@run fail(e) }
                    cleanUp()
                    setResult(Activity.RESULT_OK, Intent().putExtra(EXTRA_PATH, out.absolutePath))
                    finish()
                }
            }
        }
    }

    private fun close() {
        if (pages.isEmpty()) {
            cleanUp()
            finish()
            return
        }
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.scan_discard_title)
            .setMessage(R.string.scan_discard)
            .setPositiveButton(R.string.discard) { _, _ ->
                cleanUp()
                finish()
            }
            .setNegativeButton(R.string.cancel, null)
            .show()
    }

    private fun cleanUp() {
        Io.run({ dir.deleteRecursively() }) {}
    }

    // -----------------------------------------------------------------------
    // State
    // -----------------------------------------------------------------------

    private fun working(on: Boolean) {
        busy = on
        progress.isVisible = on
        updateUi()
    }

    private fun updateUi() {
        val p = pages.getOrNull(current)
        val has = p != null
        empty.isVisible = !has && !busy
        crop.isVisible = has && cropping
        preview.isVisible = has && !cropping
        filters.isVisible = has && !cropping
        rotate.isVisible = has && cropping
        retake.isVisible = has
        retake.setIconResource(if (cropping) R.drawable.ic_scan else R.drawable.ic_edit)
        val retakeText = getString(if (cropping) R.string.scan_retake else R.string.scan_adjust)
        retake.contentDescription = retakeText
        retake.tooltipText = retakeText
        delete.isVisible = has
        add.isVisible = has && !cropping
        ocr.isVisible = has && !cropping
        next.isEnabled = !busy && has && (cropping || pages.any { it.output != null })
        next.setText(if (cropping) R.string.scan_crop else R.string.scan_save)
        hint.setText(if (!has) R.string.scan_empty_hint else if (cropping) R.string.scan_hint_crop else R.string.scan_hint_result)
        toolbar.subtitle = if (has) getString(R.string.scan_page_of, current + 1, pages.size) else null
        toolbar.menu.findItem(R.id.scan_previous)?.isVisible = current > 0
        toolbar.menu.findItem(R.id.scan_following)?.isVisible = current in 0 until pages.size - 1
        if (p != null && !cropping) {
            filters.check(
                when (p.filter) {
                    ScanFilter.GRAY -> R.id.filter_gray
                    ScanFilter.BLACK_WHITE -> R.id.filter_bw
                    ScanFilter.COLOR -> R.id.filter_color
                },
            )
        }
    }

    override fun onSaveInstanceState(out: Bundle) {
        super.onSaveInstanceState(out)
        out.putStringArray(STATE_PHOTOS, pages.map { it.photo.absolutePath }.toTypedArray())
        out.putFloatArray(STATE_CORNERS, pages.flatMap { it.corners.asList() }.toFloatArray())
        out.putIntArray(STATE_TURNS, pages.map { it.quarter }.toIntArray())
        out.putBooleanArray(STATE_MANUAL, pages.map { it.manual }.toBooleanArray())
        out.putStringArray(STATE_OUTPUTS, pages.map { it.output?.absolutePath.orEmpty() }.toTypedArray())
        out.putIntArray(STATE_SIZES, pages.flatMap { listOf(it.width, it.height) }.toIntArray())
        out.putStringArray(STATE_FILTERS, pages.map { it.filter.name }.toTypedArray())
        out.putString(STATE_PENDING, pendingPhoto?.absolutePath)
        out.putInt(STATE_CURRENT, current)
        out.putInt(STATE_REPLACING, replacing)
    }

    private fun restore(s: Bundle) {
        val photos = s.getStringArray(STATE_PHOTOS).orEmpty()
        val corners = s.getFloatArray(STATE_CORNERS) ?: FloatArray(0)
        val turns = s.getIntArray(STATE_TURNS) ?: IntArray(0)
        val outputs = s.getStringArray(STATE_OUTPUTS).orEmpty()
        val sizes = s.getIntArray(STATE_SIZES) ?: IntArray(0)
        val filterNames = s.getStringArray(STATE_FILTERS).orEmpty()
        val manual = s.getBooleanArray(STATE_MANUAL) ?: BooleanArray(0)
        for (i in photos.indices) {
            val p = Page(File(photos[i]), corners.copyOfRange(8 * i, 8 * i + 8), turns.getOrElse(i) { 0 }, runCatching { ScanFilter.valueOf(filterNames[i]) }.getOrDefault(ScanFilter.COLOR))
            p.manual = manual.getOrElse(i) { false }
            outputs.getOrNull(i)?.takeIf { it.isNotEmpty() && File(it).exists() }?.let {
                p.output = File(it)
                p.width = sizes.getOrElse(2 * i) { 0 }
                p.height = sizes.getOrElse(2 * i + 1) { 0 }
            }
            pages.add(p)
        }
        pendingPhoto = s.getString(STATE_PENDING)?.let { File(it) }
        replacing = s.getInt(STATE_REPLACING, -1)
        val c = s.getInt(STATE_CURRENT, -1)
        if (c in pages.indices) show(c)
    }

    private fun fail(e: Throwable) {
        Toast.makeText(this, getString(R.string.scan_error, e.message ?: e.toString()), Toast.LENGTH_LONG).show()
        updateUi()
    }

    companion object {
        /** The scanned PDF (in the app's cache). */
        const val EXTRA_PATH = "io.github.blzkz.formpdfreader.SCAN_PATH"

        /** Test builds: start with this photo (a file the app can read). */
        const val EXTRA_TEST_IMAGE = "io.github.blzkz.formpdfreader.SCAN_TEST_IMAGE"

        /** Start with a photo from the gallery instead of the camera. */
        const val EXTRA_GALLERY = "io.github.blzkz.formpdfreader.SCAN_GALLERY"

        private val DEFAULT_CORNERS = floatArrayOf(0.05f, 0.05f, 0.95f, 0.05f, 0.95f, 0.95f, 0.05f, 0.95f)
        private const val DETECT_SIDE = 400
        private const val PHOTO_SIDE = 2400
        private const val OUT_SIDE = 2200f
        private const val JPEG_QUALITY = 85

        private const val STATE_PHOTOS = "photos"
        private const val STATE_CORNERS = "corners"
        private const val STATE_TURNS = "turns"
        private const val STATE_MANUAL = "manual"
        private const val STATE_OUTPUTS = "outputs"
        private const val STATE_SIZES = "sizes"
        private const val STATE_FILTERS = "filters"
        private const val STATE_PENDING = "pending"
        private const val STATE_CURRENT = "current"
        private const val STATE_REPLACING = "replacing"

        /** How sure the model has to be to turn a page by itself. */
        private const val UPRIGHT_CONFIDENCE = 0.6f

        @Volatile
        private var orientation: OrientationModel? = null

        /** The model, loaded from the assets the first time (null if it fails). */
        private fun orientationModel(c: Context): OrientationModel? =
            orientation ?: synchronized(this) {
                orientation ?: runCatching {
                    OrientationModel.load(c.assets.open("models/doc_ori.onnx").use { it.readBytes() })
                }.onFailure { android.util.Log.w("ScanActivity", "orientation model", it) }.getOrNull().also { orientation = it }
            }

        /** [page] turned upright if the model is sure of how it is turned. */
        fun upright(c: Context, page: Bitmap): Bitmap {
            val model = orientationModel(c) ?: return page
            val s = min(1f, 600f / max(page.width, page.height))
            val small = Bitmap.createScaledBitmap(page, max(1, (page.width * s).roundToInt()), max(1, (page.height * s).roundToInt()), true)
            val rgba = ByteBuffer.allocate(small.width * small.height * 4).also { small.copyPixelsToBuffer(it) }.array()
            val u = runCatching { model.upright(rgba, small.width.toUInt(), small.height.toUInt()) }.getOrNull() ?: return page
            if (u.turns.toInt() == 0 || u.confidence < UPRIGHT_CONFIDENCE) return page
            val m = Matrix().apply { postRotate(90f * u.turns.toInt()) }
            return Bitmap.createBitmap(page, 0, 0, page.width, page.height, m, true)
        }

        fun intent(c: Context, gallery: Boolean = false): Intent = Intent(c, ScanActivity::class.java).putExtra(EXTRA_GALLERY, gallery)

        /** The sheet in [photo] (corners in fractions) as an upright rectangle. */
        fun straighten(photo: Bitmap, corners: FloatArray): Bitmap {
            val w = photo.width
            val h = photo.height
            val src = FloatArray(8) { i -> if (i % 2 == 0) corners[i] * w else corners[i] * h }
            fun dist(a: Int, b: Int) = hypot(src[2 * a] - src[2 * b], src[2 * a + 1] - src[2 * b + 1])
            var ow = max(dist(0, 1), dist(3, 2))
            var oh = max(dist(0, 3), dist(1, 2))
            val s = min(1f, OUT_SIDE / max(ow, oh))
            ow = max(1f, ow * s)
            oh = max(1f, oh * s)
            val dst = floatArrayOf(0f, 0f, ow, 0f, ow, oh, 0f, oh)
            val m = Matrix()
            m.setPolyToPoly(src, 0, dst, 0, 4)
            val out = Bitmap.createBitmap(ow.roundToInt(), oh.roundToInt(), Bitmap.Config.ARGB_8888)
            Canvas(out).apply {
                drawColor(Color.WHITE)
                drawBitmap(photo, m, Paint(Paint.FILTER_BITMAP_FLAG or Paint.ANTI_ALIAS_FLAG))
            }
            return out
        }
    }
}
