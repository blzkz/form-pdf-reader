package io.github.blzkz.formpdfreader

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.RectF
import android.os.Handler
import android.os.Looper
import android.text.InputType
import android.util.LruCache
import android.view.GestureDetector
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import android.widget.OverScroller
import com.google.android.material.color.MaterialColors
import java.nio.ByteBuffer
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import uniffi.form_pdf_reader_ffi.InputResult
import uniffi.form_pdf_reader_ffi.Key
import uniffi.form_pdf_reader_ffi.PdfDocument

/**
 * Shows the pages of a [PdfDocument] one below the other and sends taps and
 * typing to the form.
 *
 * Pages are drawn in horizontal tiles rendered on the PDFium thread
 * ([PdfEngine]). While zooming, the existing tiles are stretched until the
 * new ones arrive.
 */
class PdfPageView(context: Context) : View(context) {

    /** Receives what happened after each input on the form. */
    var onInput: ((InputResult) -> Unit)? = null

    private class PageInfo(val widthPt: Float, val heightPt: Float, val shownHeightPt: Float)

    private data class TileKey(val page: Int, val index: Int)

    private class Tile(val bitmap: Bitmap, val scale: Float, val generation: Int)

    private var doc: PdfDocument? = null
    private var pages: List<PageInfo> = emptyList()

    /** Pixels per PDF point at zoom 1 (page width fills the view). */
    private var fitScale = 1f
    private var zoom = 1f
    private val scale get() = fitScale * zoom

    private var offsetX = 0f
    private var offsetY = 0f

    /** Increases every time the pages change: tiles of older generations are redrawn. */
    private var generation = 0
    private val pending = HashSet<TileKey>()
    private val tiles = object : LruCache<TileKey, Tile>((Runtime.getRuntime().maxMemory() / 6).toInt()) {
        override fun sizeOf(key: TileKey, value: Tile) = value.bitmap.byteCount
    }

    private val density = resources.displayMetrics.density
    private val margin = 12f * density
    private val gap = 12f * density
    private val tileHeight = 512

    private val main = Handler(Looper.getMainLooper())
    private val scroller = OverScroller(context)
    private var scaling = false

    private val pagePaint = Paint().apply { color = Color.WHITE }
    private val shadowPaint = Paint().apply { color = Color.argb(50, 0, 0, 0) }
    private val bitmapPaint = Paint(Paint.FILTER_BITMAP_FLAG)
    /** Around the pages: a surface colour of the theme (light or dark). */
    private val background = MaterialColors.getColor(
        context, com.google.android.material.R.attr.colorSurfaceContainer, Color.rgb(0xE6, 0xE6, 0xE6),
    )

    /** Text being composed by the keyboard and already sent to the form. */
    private var composing = ""

    init {
        isFocusable = true
        isFocusableInTouchMode = true
    }

    // -----------------------------------------------------------------------
    // Document
    // -----------------------------------------------------------------------

    /** Shows [document]. Page sizes are read on the PDFium thread. */
    fun setDocument(document: PdfDocument?) {
        doc = document
        tiles.evictAll()
        pending.clear()
        pages = emptyList()
        offsetX = 0f
        offsetY = 0f
        zoom = 1f
        generation++
        if (document == null) {
            invalidate()
            return
        }
        reloadPages()
    }

    /** Reads the page sizes again (the form may have grown or shrunk). */
    fun reloadPages() {
        val d = doc ?: return
        PdfEngine.run({
            (0u until d.pageCount()).map { p ->
                val s = d.pageSize(p)
                PageInfo(s.width, s.height, d.contentHeight(p))
            }
        }) { r ->
            if (d !== doc) return@run
            r.onSuccess {
                pages = it
                updateFitScale()
                clampOffsets()
                redraw()
            }
        }
    }

    /** Draws the pages again (after the form changed). */
    fun redraw() {
        generation++
        invalidate()
    }

    // -----------------------------------------------------------------------
    // Layout
    // -----------------------------------------------------------------------

    private fun updateFitScale() {
        val maxW = pages.maxOfOrNull { it.widthPt } ?: return
        if (width > 0 && maxW > 0) fitScale = (width - 2 * margin) / maxW
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        val old = scale
        updateFitScale()
        if (old != scale) generation++
        clampOffsets()
    }

    private fun contentHeight(): Float =
        2 * margin + pages.sumOf { (it.shownHeightPt * scale).toDouble() }.toFloat() + gap * max(0, pages.size - 1)

    private fun contentWidth(): Float = 2 * margin + (pages.maxOfOrNull { it.widthPt } ?: 0f) * scale

    private fun clampOffsets() {
        offsetY = offsetY.coerceIn(0f, max(0f, contentHeight() - height))
        offsetX = offsetX.coerceIn(0f, max(0f, contentWidth() - width))
    }

    /** Rectangle of page [i] on screen. */
    private fun pageRect(i: Int): RectF {
        var top = margin - offsetY
        for (k in 0 until i) top += pages[k].shownHeightPt * scale + gap
        val p = pages[i]
        val w = p.widthPt * scale
        val left = if (w + 2 * margin <= width) (width - w) / 2f else margin - offsetX
        return RectF(left, top, left + w, top + p.shownHeightPt * scale)
    }

    // -----------------------------------------------------------------------
    // Drawing
    // -----------------------------------------------------------------------

    override fun onDraw(canvas: Canvas) {
        canvas.drawColor(background)
        val d = doc ?: return
        if (pages.isEmpty()) return
        val s = scale
        for (i in pages.indices) {
            val r = pageRect(i)
            if (r.bottom < 0 || r.top > height) continue
            canvas.drawRect(r.left + 2 * density, r.top + 3 * density, r.right + 2 * density, r.bottom + 3 * density, shadowPaint)
            canvas.drawRect(r, pagePaint)
            drawTiles(canvas, d, i, r, s)
        }
    }

    private fun drawTiles(canvas: Canvas, d: PdfDocument, page: Int, r: RectF, s: Float) {
        val shownPx = (pages[page].shownHeightPt * s).roundToInt()
        val count = (shownPx + tileHeight - 1) / tileHeight
        for (t in 0 until count) {
            val top = r.top + t * tileHeight
            val bottom = min(r.top + (t + 1) * tileHeight, r.bottom)
            if (bottom < -tileHeight || top > height + tileHeight) continue
            val key = TileKey(page, t)
            val tile = tiles.get(key)
            if (tile != null) {
                // A tile rendered at another zoom is stretched to the current one.
                val f = s / tile.scale
                val dstTop = r.top + t * tileHeight * f
                val dst = RectF(r.left, dstTop, r.left + tile.bitmap.width * f, dstTop + tile.bitmap.height * f)
                canvas.drawBitmap(tile.bitmap, null, dst, bitmapPaint)
            }
            val stale = tile == null || tile.scale != s || tile.generation != generation
            if (stale && !scaling) requestTile(d, key, s)
        }
    }

    private fun requestTile(d: PdfDocument, key: TileKey, s: Float) {
        if (key in pending || pending.size > 8) return
        pending.add(key)
        val gen = generation
        val p = pages[key.page]
        val pageW = (p.widthPt * s).roundToInt().coerceAtLeast(1)
        val pageH = (p.heightPt * s).roundToInt().coerceAtLeast(1)
        val shown = (p.shownHeightPt * s).roundToInt()
        val y = key.index * tileHeight
        val h = min(tileHeight, shown - y)
        if (h <= 0) {
            pending.remove(key)
            return
        }
        PdfEngine.run({ d.render(key.page.toUInt(), pageW, pageH, 0, y, pageW, h) }) { r ->
            pending.remove(key)
            if (d !== doc) return@run
            val px = r.getOrNull()
            if (px != null && px.size == pageW * h * 4) {
                val bmp = Bitmap.createBitmap(pageW, h, Bitmap.Config.ARGB_8888)
                bmp.copyPixelsFromBuffer(ByteBuffer.wrap(px))
                tiles.put(key, Tile(bmp, s, gen))
            }
            invalidate()
        }
    }

    // -----------------------------------------------------------------------
    // Gestures
    // -----------------------------------------------------------------------

    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(e: MotionEvent): Boolean {
            scroller.forceFinished(true)
            return true
        }

        override fun onScroll(e1: MotionEvent?, e2: MotionEvent, dx: Float, dy: Float): Boolean {
            offsetX += dx
            offsetY += dy
            clampOffsets()
            invalidate()
            return true
        }

        override fun onFling(e1: MotionEvent?, e2: MotionEvent, vx: Float, vy: Float): Boolean {
            scroller.fling(
                offsetX.roundToInt(), offsetY.roundToInt(), -vx.roundToInt(), -vy.roundToInt(),
                0, max(0, (contentWidth() - width).roundToInt()), 0, max(0, (contentHeight() - height).roundToInt()),
            )
            postInvalidateOnAnimation()
            return true
        }

        override fun onSingleTapUp(e: MotionEvent): Boolean {
            tap(e.x, e.y)
            return true
        }
    })

    private val scaleGestures = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
        override fun onScaleBegin(detector: ScaleGestureDetector): Boolean {
            scaling = true
            return true
        }

        override fun onScale(detector: ScaleGestureDetector): Boolean {
            val old = zoom
            zoom = (zoom * detector.scaleFactor).coerceIn(0.5f, 6f)
            val f = zoom / old
            // Keep the point under the fingers in place.
            offsetX = (offsetX + detector.focusX) * f - detector.focusX
            offsetY = (offsetY + detector.focusY) * f - detector.focusY
            clampOffsets()
            invalidate()
            return true
        }

        override fun onScaleEnd(detector: ScaleGestureDetector) {
            scaling = false
            invalidate()
        }
    })

    override fun onTouchEvent(event: MotionEvent): Boolean {
        scaleGestures.onTouchEvent(event)
        if (!scaleGestures.isInProgress) gestures.onTouchEvent(event)
        return true
    }

    override fun computeScroll() {
        if (scroller.computeScrollOffset()) {
            offsetX = scroller.currX.toFloat()
            offsetY = scroller.currY.toFloat()
            clampOffsets()
            postInvalidateOnAnimation()
        }
    }

    // -----------------------------------------------------------------------
    // Form input
    // -----------------------------------------------------------------------

    private fun tap(x: Float, y: Float) {
        val d = doc ?: return
        for (i in pages.indices) {
            val r = pageRect(i)
            if (!r.contains(x, y)) continue
            val s = scale.toDouble()
            val p = pages[i]
            val pageW = p.widthPt * s
            val pageH = p.heightPt * s
            val lx = (x - r.left).toDouble()
            val ly = (y - r.top).toDouble()
            requestFocus()
            commitComposing()
            send { d.tap(i.toUInt(), pageW, pageH, lx, ly) }
            return
        }
    }

    /** Sends an input to the form and handles its result on the UI thread. */
    private fun send(block: () -> InputResult) {
        PdfEngine.run(block) { r -> r.onSuccess { handleResult(it, true) } }
    }

    private fun handleResult(r: InputResult, fromUser: Boolean) {
        if (r.restructured) reloadPages() else if (r.redraw) redraw()
        if (fromUser) {
            val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
            if (r.textInput) {
                requestFocus()
                imm.restartInput(this)
                imm.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
            } else {
                imm.hideSoftInputFromWindow(windowToken, 0)
            }
            // PDFium sometimes lays out one event later: check again.
            for (delay in longArrayOf(150, 450)) {
                main.postDelayed({ checkStructure() }, delay)
            }
            scheduleTimers()
        }
        onInput?.invoke(r)
    }

    private fun checkStructure() {
        val d = doc ?: return
        PdfEngine.run({ d.checkStructure() }) { r ->
            if (d !== doc) return@run
            r.onSuccess { if (it.restructured) reloadPages() }
        }
    }

    private var timersScheduled = false

    /** Runs PDFium's timers (caret blink, delayed scripts) while there are any. */
    private fun scheduleTimers() {
        val d = doc ?: return
        if (timersScheduled) return
        timersScheduled = true
        PdfEngine.run({ d.processTimers() }) { r ->
            timersScheduled = false
            val next = r.getOrNull() ?: return@run
            if (d !== doc) return@run
            redraw()
            main.postDelayed({ scheduleTimers() }, next.toLong().coerceIn(30, 1000))
        }
    }

    /** Commits the field being edited (before saving). */
    fun commitField(after: () -> Unit) {
        val d = doc ?: return after()
        commitComposing()
        PdfEngine.run({ d.killFocus() }) { r ->
            r.onSuccess { handleResult(it, false) }
            after()
        }
    }

    private fun typeText(text: String) {
        val d = doc ?: return
        if (text.isEmpty()) return
        send { d.typeText(text) }
    }

    private fun pressKey(key: Key) {
        val d = doc ?: return
        send { d.key(key) }
    }

    private fun commitComposing() {
        composing = ""
    }

    // -----------------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------------

    override fun onCheckIsTextEditor(): Boolean = doc != null

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        // Without suggestions most keyboards send each character directly.
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or
            InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or
            InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_FLAG_NO_FULLSCREEN or EditorInfo.IME_ACTION_DONE
        return object : BaseInputConnection(this, false) {
            override fun commitText(text: CharSequence, newCursorPosition: Int): Boolean {
                replaceComposing(text.toString())
                composing = ""
                return true
            }

            override fun setComposingText(text: CharSequence, newCursorPosition: Int): Boolean {
                // Keyboards that compose words: the form receives the text as it
                // is composed, replacing the previous version.
                replaceComposing(text.toString())
                composing = text.toString()
                return true
            }

            override fun finishComposingText(): Boolean {
                composing = ""
                return true
            }

            override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                repeat(beforeLength) { pressKey(Key.BACKSPACE) }
                repeat(afterLength) { pressKey(Key.DELETE) }
                return true
            }

            override fun performEditorAction(actionCode: Int): Boolean {
                pressKey(Key.TAB)
                return true
            }

            override fun sendKeyEvent(event: KeyEvent): Boolean {
                if (event.action == KeyEvent.ACTION_DOWN) onKeyDown(event.keyCode, event)
                return true
            }
        }
    }

    /** Replaces the text being composed with [text] in the form. */
    private fun replaceComposing(text: String) {
        val common = composing.commonPrefixWith(text)
        repeat(composing.length - common.length) { pressKey(Key.BACKSPACE) }
        typeText(text.substring(common.length))
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        val key = when (keyCode) {
            KeyEvent.KEYCODE_DEL -> Key.BACKSPACE
            KeyEvent.KEYCODE_FORWARD_DEL -> Key.DELETE
            KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> Key.ENTER
            KeyEvent.KEYCODE_TAB -> Key.TAB
            KeyEvent.KEYCODE_DPAD_LEFT -> Key.LEFT
            KeyEvent.KEYCODE_DPAD_RIGHT -> Key.RIGHT
            KeyEvent.KEYCODE_DPAD_UP -> Key.UP
            KeyEvent.KEYCODE_DPAD_DOWN -> Key.DOWN
            KeyEvent.KEYCODE_MOVE_HOME -> Key.HOME
            KeyEvent.KEYCODE_MOVE_END -> Key.END
            else -> null
        }
        if (key != null && doc != null) {
            pressKey(key)
            return true
        }
        val c = event.unicodeChar
        if (c != 0 && doc != null) {
            typeText(String(Character.toChars(c)))
            return true
        }
        return super.onKeyDown(keyCode, event)
    }
}
