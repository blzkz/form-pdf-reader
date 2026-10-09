package io.github.blzkz.formpdfreader

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.res.Configuration
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.PointF
import android.graphics.Rect
import android.graphics.RectF
import android.graphics.drawable.Drawable
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.text.InputType
import android.util.LruCache
import android.view.ActionMode
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.InputDevice
import android.view.KeyEvent
import android.view.Menu
import android.view.MenuItem
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.view.ViewConfiguration
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import android.widget.OverScroller
import android.widget.Toast
import com.google.android.material.color.MaterialColors
import java.nio.ByteBuffer
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.roundToInt
import uniffi.form_pdf_reader_ffi.InputResult
import uniffi.form_pdf_reader_ffi.Key
import uniffi.form_pdf_reader_ffi.PdfDocument

/**
 * Shows the pages of a [PdfDocument] and sends taps and typing to the form.
 *
 * Pages are laid out one below the other, one at a time or in pairs
 * ([Layout]), and drawn in tiles rendered on the PDFium thread
 * ([PdfEngine]). While zooming, the existing tiles are stretched until the
 * new ones arrive.
 *
 * Text is selected with a long press (then with the handles) or by dragging
 * with a mouse; the floating toolbar copies it.
 */
class PdfPageView(context: Context) : View(context) {

    enum class Layout { CONTINUOUS, SINGLE, TWO_PAGES }

    /** How the zoom follows the size of the view. */
    enum class Fit { WIDTH, PAGE, NONE }

    /** Receives what happened after each input on the form. */
    var onInput: ((InputResult) -> Unit)? = null

    /** The current page, the page count, the zoom or the layout changed. */
    var onStateChanged: (() -> Unit)? = null

    private class PageInfo(val widthPt: Float, val heightPt: Float, val shownHeightPt: Float)

    /** Where a page is, in pixels of the whole content. */
    private class Slot(val page: Int, val x: Float, val y: Float, val w: Float, val h: Float)

    private data class TileKey(val page: Int, val row: Int, val col: Int)

    private class Tile(val bitmap: Bitmap, val scale: Float, val generation: Int)

    /** A point of a page that has to stay at (viewX, viewY) after a relayout. */
    private class Anchor(val page: Int, val fx: Float, val fy: Float, val viewX: Float, val viewY: Float)

    /** Selected characters [min, max] of a page; [anchor] stays while [focus] moves. */
    private class Selection(val page: Int, var anchor: Int, var focus: Int) {
        val start get() = min(anchor, focus)
        val end get() = max(anchor, focus)
    }

    private enum class Handle { START, END }

    private var doc: PdfDocument? = null
    private var pages: List<PageInfo> = emptyList()
    private var slots: List<Slot> = emptyList()
    private var contentW = 0f
    private var contentH = 0f

    var layoutMode = Layout.CONTINUOUS
        private set
    var fit = Fit.WIDTH
        private set

    /**
     * The layout is chosen when the document is first shown: two pages side
     * by side in landscape (if it has more than one), otherwise continuous.
     */
    private var initialLayoutPending = true

    /** Pixels per PDF point. */
    private var scale = 1f

    /** Page shown in single-page view. */
    private var single = 0

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

    /** Scale at 100 % zoom: the page at its printed size. */
    private val pointScale = density * 160f / 72f
    private val minScale = pointScale * 0.1f
    private val maxScale = pointScale * 8f

    private val main = Handler(Looper.getMainLooper())
    private val scroller = OverScroller(context)
    private var scaling = false
    private val touchSlop = ViewConfiguration.get(context).scaledTouchSlop

    private val pagePaint = Paint().apply { color = Color.WHITE }
    private val shadowPaint = Paint().apply { color = Color.argb(50, 0, 0, 0) }
    private val bitmapPaint = Paint(Paint.FILTER_BITMAP_FLAG)

    /** Around the pages: a surface colour of the theme (light or dark). */
    private val background = MaterialColors.getColor(
        context, com.google.android.material.R.attr.colorSurfaceContainer, Color.rgb(0xE6, 0xE6, 0xE6),
    )
    private val accent = MaterialColors.getColor(context, androidx.appcompat.R.attr.colorPrimary, Color.rgb(0x9A, 0x2A, 0x2A))
    private val selectionPaint = Paint().apply { color = Color.argb(0x60, Color.red(accent), Color.green(accent), Color.blue(accent)) }

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
        clearSelection()
        doc = document
        tiles.evictAll()
        pending.clear()
        pages = emptyList()
        slots = emptyList()
        offsetX = 0f
        offsetY = 0f
        single = 0
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
                single = single.coerceIn(0, max(0, pages.size - 1))
                if (initialLayoutPending && width > 0) chooseInitialLayout()
                relayout(null)
                redraw()
                notifyState()
            }
        }
    }

    /** Draws the pages again (after the form changed). */
    fun redraw() {
        generation++
        invalidate()
    }

    /** Frees the rendered tiles (the view is hidden). */
    fun trimMemory() {
        tiles.evictAll()
    }

    // -----------------------------------------------------------------------
    // State for the toolbar
    // -----------------------------------------------------------------------

    val pageCount: Int get() = pages.size

    /** The page at the top of the view (0-based). */
    val currentPage: Int
        get() {
            if (layoutMode == Layout.SINGLE) return single
            var best = slots.firstOrNull() ?: return 0
            val line = offsetY + height * 0.4f
            for (s in slots) if (s.y <= line && s.y > best.y) best = s
            return best.page
        }

    val zoomPercent: Int get() = (scale / pointScale * 100).roundToInt()

    private var lastState = ""

    private fun notifyState() {
        val s = "$currentPage/${pages.size}/$zoomPercent/$layoutMode/$fit"
        if (s != lastState) {
            lastState = s
            onStateChanged?.invoke()
        }
    }

    // -----------------------------------------------------------------------
    // Layout
    // -----------------------------------------------------------------------

    private fun rows(): List<List<Int>> = when (layoutMode) {
        Layout.CONTINUOUS -> pages.indices.map { listOf(it) }
        Layout.TWO_PAGES -> pages.indices.chunked(2)
        Layout.SINGLE -> listOf(listOf(single))
    }

    /** Height used to fit a page: a continuous XFA form is one very long page. */
    private fun fitHeightPt(p: PageInfo) = min(p.shownHeightPt, p.widthPt * 1.42f)

    private fun fitScale(f: Fit): Float {
        var best = Float.MAX_VALUE
        for (row in rows()) {
            val sumW = row.sumOf { pages[it].widthPt.toDouble() }.toFloat()
            if (sumW <= 0f) continue
            var s = (width - 2 * margin - gap * (row.size - 1)) / sumW
            if (f == Fit.PAGE) {
                val h = row.maxOf { fitHeightPt(pages[it]) }
                if (h > 0f) s = min(s, (height - 2 * margin) / h)
            }
            best = min(best, s)
        }
        return if (best == Float.MAX_VALUE) scale else best.coerceIn(minScale, maxScale)
    }

    private fun chooseInitialLayout() {
        initialLayoutPending = false
        val landscape = resources.configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
        layoutMode = if (landscape && pages.size > 1) Layout.TWO_PAGES else Layout.CONTINUOUS
        single = 0
        fit = Fit.WIDTH
        offsetX = 0f
        offsetY = 0f
    }

    /** Places the pages; [anchor] keeps a point of a page in place. */
    private fun relayout(anchor: Anchor?) {
        if (pages.isEmpty() || width == 0 || height == 0) {
            slots = emptyList()
            return
        }
        if (fit != Fit.NONE) scale = fitScale(fit)
        val rows = rows()
        val rowWidths = rows.map { row -> row.sumOf { (pages[it].widthPt * scale).toDouble() }.toFloat() + gap * (row.size - 1) }
        contentW = max(width.toFloat(), (rowWidths.maxOrNull() ?: 0f) + 2 * margin)
        var y = margin
        if (layoutMode == Layout.SINGLE) {
            // A page smaller than the view is centred vertically.
            val h = pages[single].shownHeightPt * scale
            y = max(margin, (height - h) / 2)
        }
        val out = ArrayList<Slot>(pages.size)
        for ((i, row) in rows.withIndex()) {
            var x = (contentW - rowWidths[i]) / 2
            var rowH = 0f
            for (p in row) {
                val w = pages[p].widthPt * scale
                val h = pages[p].shownHeightPt * scale
                out += Slot(p, x, y, w, h)
                x += w + gap
                rowH = max(rowH, h)
            }
            y += rowH + gap
        }
        contentH = y - gap + margin
        if (layoutMode == Layout.SINGLE) contentH = max(contentH, height.toFloat())
        slots = out
        if (anchor != null) restore(anchor)
        clampOffsets()
    }

    private fun slotOf(page: Int): Slot? = slots.firstOrNull { it.page == page }

    private fun slotAt(x: Float, y: Float): Slot? {
        val cx = x + offsetX
        val cy = y + offsetY
        return slots.firstOrNull { cx >= it.x && cx <= it.x + it.w && cy >= it.y && cy <= it.y + it.h }
    }

    /** The point of the nearest page under (vx, vy) on the view. */
    private fun anchorAt(vx: Float, vy: Float, toX: Float = vx, toY: Float = vy): Anchor? {
        val cx = vx + offsetX
        val cy = vy + offsetY
        val s = slots.minByOrNull {
            max(0f, max(it.x - cx, cx - it.x - it.w)) + max(0f, max(it.y - cy, cy - it.y - it.h))
        } ?: return null
        return Anchor(s.page, (cx - s.x) / s.w, (cy - s.y) / s.h, toX, toY)
    }

    private fun restore(a: Anchor) {
        val s = slotOf(a.page) ?: return
        offsetX = s.x + a.fx * s.w - a.viewX
        offsetY = s.y + a.fy * s.h - a.viewY
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        if (initialLayoutPending && pages.isNotEmpty()) chooseInitialLayout()
        // After a rotation, the line at the top stays at the top.
        val a = if (oldw > 0) anchorAt(oldw / 2f, 0f, w / 2f, 0f) else null
        relayout(a)
        notifyState()
    }

    private fun clampOffsets() {
        offsetY = offsetY.coerceIn(0f, max(0f, contentH - height))
        offsetX = offsetX.coerceIn(0f, max(0f, contentW - width))
    }

    // -----------------------------------------------------------------------
    // Navigation and zoom
    // -----------------------------------------------------------------------

    fun setLayout(mode: Layout) {
        initialLayoutPending = false
        if (mode == layoutMode) return
        val page = currentPage
        layoutMode = mode
        single = page
        relayout(null)
        goToPage(page)
    }

    fun setFitMode(f: Fit) {
        fit = f
        relayout(anchorAt(width / 2f, 0f))
        invalidate()
        notifyState()
    }

    /** Zooms keeping the point (vx, vy) of the view in place. */
    fun zoomTo(newScale: Float, vx: Float = width / 2f, vy: Float = height / 2f) {
        val a = anchorAt(vx, vy)
        fit = Fit.NONE
        scale = newScale.coerceIn(minScale, maxScale)
        relayout(a)
        actionMode?.invalidateContentRect()
        invalidate()
        notifyState()
    }

    fun setZoomPercent(percent: Int) = zoomTo(percent * pointScale / 100)

    fun zoomIn() {
        val p = zoomPercent
        setZoomPercent(ZOOM_STEPS.firstOrNull { it > p + 1 } ?: ZOOM_STEPS.last())
    }

    fun zoomOut() {
        val p = zoomPercent
        setZoomPercent(ZOOM_STEPS.lastOrNull { it < p - 1 } ?: ZOOM_STEPS.first())
    }

    fun goToPage(page: Int) {
        if (pages.isEmpty()) return
        val target = page.coerceIn(0, pages.size - 1)
        if (layoutMode == Layout.SINGLE) {
            if (target != single) clearSelection()
            single = target
            relayout(null)
            offsetY = 0f
            offsetX = (contentW - width) / 2
        } else {
            val s = slotOf(target) ?: return
            offsetY = s.y - margin
        }
        scroller.forceFinished(true)
        clampOffsets()
        actionMode?.invalidateContentRect()
        invalidate()
        notifyState()
    }

    private val pageStep get() = if (layoutMode == Layout.TWO_PAGES) 2 else 1

    fun nextPage() = goToPage(currentPage - currentPage % pageStep + pageStep)

    fun previousPage() = goToPage(currentPage - currentPage % pageStep - pageStep)

    private fun scrollByScreen(direction: Int) {
        if (layoutMode == Layout.SINGLE) {
            if (direction > 0) nextPage() else previousPage()
            return
        }
        scrollContent(0f, direction * (height - 48 * density))
    }

    private fun scrollContent(dx: Float, dy: Float) {
        offsetX += dx
        offsetY += dy
        clampOffsets()
        actionMode?.invalidateContentRect()
        invalidate()
        notifyState()
    }

    // -----------------------------------------------------------------------
    // Drawing
    // -----------------------------------------------------------------------

    override fun onDraw(canvas: Canvas) {
        canvas.drawColor(background)
        val d = doc ?: return
        val s = scale
        for (slot in slots) {
            val l = slot.x - offsetX
            val t = slot.y - offsetY
            if (t > height || t + slot.h < 0 || l > width || l + slot.w < 0) continue
            val r = RectF(l, t, l + slot.w, t + slot.h)
            canvas.drawRect(r.left + 2 * density, r.top + 3 * density, r.right + 2 * density, r.bottom + 3 * density, shadowPaint)
            canvas.drawRect(r, pagePaint)
            drawTiles(canvas, d, slot.page, r, s)
        }
        drawSelection(canvas)
    }

    private fun drawTiles(canvas: Canvas, d: PdfDocument, page: Int, r: RectF, s: Float) {
        val p = pages[page]
        val pageW = (p.widthPt * s).roundToInt()
        val shown = (p.shownHeightPt * s).roundToInt()
        val rows = (shown + TILE_H - 1) / TILE_H
        val cols = (pageW + TILE_W - 1) / TILE_W
        if (rows == 0 || cols == 0) return
        // Visible tiles, and the next row to scroll smoothly.
        val r0 = ((-r.top) / TILE_H).toInt().coerceIn(0, rows - 1)
        val r1 = ((height - r.top) / TILE_H).toInt().plus(1).coerceIn(0, rows - 1)
        val c0 = ((-r.left) / TILE_W).toInt().coerceIn(0, cols - 1)
        val c1 = ((width - r.left) / TILE_W).toInt().coerceIn(0, cols - 1)
        canvas.save()
        canvas.clipRect(r)
        for (row in r0..r1) {
            for (col in c0..c1) {
                val key = TileKey(page, row, col)
                val tile = tiles.get(key)
                if (tile != null) {
                    // A tile rendered at another zoom is stretched to the current one.
                    val f = s / tile.scale
                    val left = r.left + col * TILE_W * f
                    val top = r.top + row * TILE_H * f
                    canvas.drawBitmap(tile.bitmap, null, RectF(left, top, left + tile.bitmap.width * f, top + tile.bitmap.height * f), bitmapPaint)
                }
                val stale = tile == null || tile.scale != s || tile.generation != generation
                if (stale && !scaling) requestTile(d, key, s)
            }
        }
        canvas.restore()
    }

    private fun requestTile(d: PdfDocument, key: TileKey, s: Float) {
        if (key in pending || pending.size > 8) return
        val gen = generation
        val p = pages[key.page]
        val pageW = (p.widthPt * s).roundToInt().coerceAtLeast(1)
        val pageH = (p.heightPt * s).roundToInt().coerceAtLeast(1)
        val shown = (p.shownHeightPt * s).roundToInt()
        val x = key.col * TILE_W
        val y = key.row * TILE_H
        val w = min(TILE_W, pageW - x)
        val h = min(TILE_H, shown - y)
        if (w <= 0 || h <= 0) return
        pending.add(key)
        PdfEngine.run({ d.render(key.page.toUInt(), pageW, pageH, x, y, w, h) }) { r ->
            pending.remove(key)
            if (d !== doc) return@run
            val px = r.getOrNull()
            if (px != null && px.size == w * h * 4) {
                val bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
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
            scrollContent(dx, dy)
            return true
        }

        override fun onFling(e1: MotionEvent?, e2: MotionEvent, vx: Float, vy: Float): Boolean {
            // Single page that fits the width: a horizontal swipe turns the page.
            if (layoutMode == Layout.SINGLE && contentW <= width + 1 && abs(vx) > 2 * abs(vy)) {
                if (vx < 0) nextPage() else previousPage()
                return true
            }
            scroller.fling(
                offsetX.roundToInt(), offsetY.roundToInt(), -vx.roundToInt(), -vy.roundToInt(),
                0, max(0, (contentW - width).roundToInt()), 0, max(0, (contentH - height).roundToInt()),
            )
            postInvalidateOnAnimation()
            return true
        }

        override fun onSingleTapUp(e: MotionEvent): Boolean {
            if (selection != null) clearSelection() else tap(e.x, e.y)
            return true
        }

        override fun onLongPress(e: MotionEvent) {
            startSelection(e.x, e.y)
        }
    })

    private val scaleGestures = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
        override fun onScaleBegin(detector: ScaleGestureDetector): Boolean {
            scaling = true
            return true
        }

        override fun onScale(detector: ScaleGestureDetector): Boolean {
            // Keep the point under the fingers in place.
            val a = anchorAt(detector.focusX, detector.focusY)
                fit = Fit.NONE
            scale = (scale * detector.scaleFactor).coerceIn(minScale, maxScale)
            relayout(a)
            actionMode?.invalidateContentRect()
            invalidate()
            return true
        }

        override fun onScaleEnd(detector: ScaleGestureDetector) {
            scaling = false
            invalidate()
            notifyState()
        }
    })

    /** A finger is on the screen (the long press result arrives later). */
    private var fingerDown = false

    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (event.isFromSource(InputDevice.SOURCE_MOUSE) && !isTouchpadGesture(event)) return onMouseEvent(event)
        fingerDown = event.actionMasked != MotionEvent.ACTION_UP && event.actionMasked != MotionEvent.ACTION_CANCEL
        // Dragging a selection handle (or extending the selection after a long press).
        if (dragging != null) {
            onHandleDrag(event)
            return true
        }
        if (event.actionMasked == MotionEvent.ACTION_DOWN) {
            val h = handleAt(event.x, event.y)
            if (h != null) {
                beginHandleDrag(h, event.x, event.y)
                return true
            }
        }
        scaleGestures.onTouchEvent(event)
        if (!scaleGestures.isInProgress) gestures.onTouchEvent(event)
        return true
    }

    override fun computeScroll() {
        if (scroller.computeScrollOffset()) {
            offsetX = scroller.currX.toFloat()
            offsetY = scroller.currY.toFloat()
            clampOffsets()
            actionMode?.invalidateContentRect()
            notifyState()
            postInvalidateOnAnimation()
        }
    }

    // -----------------------------------------------------------------------
    // Mouse and touchpad
    // -----------------------------------------------------------------------

    /**
     * Two-finger scroll and pinch on a touchpad: Android sends them as
     * fingers on the screen (from the mouse source), so they scroll and zoom
     * like touch gestures.
     */
    private fun isTouchpadGesture(e: MotionEvent): Boolean {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            val c = e.classification
            if (c == MotionEvent.CLASSIFICATION_TWO_FINGER_SWIPE || c == MotionEvent.CLASSIFICATION_PINCH) return true
        }
        return e.pointerCount > 1
    }

    private var mouseDown: PointF? = null
    private var mouseSelecting = false

    /** Click: like a tap. Drag: selects text. The wheel scrolls (see onGenericMotionEvent). */
    private fun onMouseEvent(e: MotionEvent): Boolean {
        when (e.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                if (e.isButtonPressed(MotionEvent.BUTTON_PRIMARY)) {
                    requestFocus()
                    mouseDown = PointF(e.x, e.y)
                    mouseSelecting = false
                } else if (e.isButtonPressed(MotionEvent.BUTTON_SECONDARY) && selection != null) {
                    showActionMode()
                }
            }
            MotionEvent.ACTION_MOVE -> {
                val down = mouseDown ?: return true
                if (mouseSelecting) {
                    dragTo(e.x, e.y)
                } else if (hypot(e.x - down.x, e.y - down.y) > touchSlop) {
                    mouseSelecting = true
                    clearSelection()
                    beginMouseSelection(down, e.x, e.y)
                }
            }
            MotionEvent.ACTION_UP -> {
                val down = mouseDown
                mouseDown = null
                if (mouseSelecting) {
                    mouseSelecting = false
                    showActionMode()
                } else if (down != null) {
                    if (selection != null) clearSelection() else tap(e.x, e.y)
                }
            }
            MotionEvent.ACTION_CANCEL -> {
                mouseDown = null
                mouseSelecting = false
            }
        }
        return true
    }

    override fun onGenericMotionEvent(e: MotionEvent): Boolean {
        if (e.isFromSource(InputDevice.SOURCE_CLASS_POINTER) && e.actionMasked == MotionEvent.ACTION_SCROLL) {
            val v = e.getAxisValue(MotionEvent.AXIS_VSCROLL)
            val h = e.getAxisValue(MotionEvent.AXIS_HSCROLL)
            if ((e.metaState and KeyEvent.META_CTRL_ON) != 0) {
                if (v != 0f) zoomTo(scale * 1.1f.pow(v), e.x, e.y)
                return true
            }
            val step = 64 * density
            var dx = h * step
            var dy = -v * step
            if ((e.metaState and KeyEvent.META_SHIFT_ON) != 0) {
                dx = -v * step
                dy = 0f
            }
            // Single page: scrolling past its end turns the page.
            if (layoutMode == Layout.SINGLE && dy != 0f) {
                val maxY = max(0f, contentH - height)
                if (dy > 0 && offsetY >= maxY - 1) {
                    if (single < pages.size - 1) nextPage()
                    return true
                }
                if (dy < 0 && offsetY <= 1) {
                    if (single > 0) {
                        previousPage()
                        scrollContent(0f, contentH)
                    }
                    return true
                }
            }
            scrollContent(dx, dy)
            return true
        }
        return super.onGenericMotionEvent(e)
    }

    // -----------------------------------------------------------------------
    // Text selection
    // -----------------------------------------------------------------------

    private var selection: Selection? = null

    /** Selection rectangles in pixels of the page drawn at [selectionScale]. */
    private var selectionRects: List<RectF> = emptyList()
    private var selectionScale = 1f
    private var selectionToken = 0

    /** Touch selections have handles; mouse selections do not. */
    private var showHandles = false
    private var dragging: Handle? = null
    private var dragDx = 0f
    private var dragDy = 0f
    /** Word selected by the long press: dragging afterwards never shrinks it. */
    private var longPressWord: IntRange? = null
    private var longPressPoint = PointF()
    private var charQueryBusy = false
    private var pendingDrag: PointF? = null
    private var actionMode: ActionMode? = null

    private val handleLeft: Drawable?
    private val handleRight: Drawable?

    init {
        val a = context.obtainStyledAttributes(intArrayOf(android.R.attr.textSelectHandleLeft, android.R.attr.textSelectHandleRight))
        handleLeft = a.getDrawable(0)?.mutate()?.apply { setTint(accent) }
        handleRight = a.getDrawable(1)?.mutate()?.apply { setTint(accent) }
        a.recycle()
    }

    val hasSelection: Boolean get() = selection != null

    fun clearSelection() {
        selection = null
        selectionRects = emptyList()
        selectionToken++
        dragging = null
        longPressWord = null
        pendingDrag = null
        actionMode?.finish()
        actionMode = null
        invalidate()
    }

    /** Character at (x, y) of the view on [page], or -1, delivered on the UI thread. */
    private fun charAt(page: Int, x: Float, y: Float, then: (Int) -> Unit) {
        val d = doc ?: return
        val slot = slotOf(page) ?: return
        val p = pages[page]
        val s = scale
        val lx = (x - (slot.x - offsetX)).toDouble()
        val ly = (y - (slot.y - offsetY)).toDouble()
        PdfEngine.run({ d.textCharAt(page.toUInt(), (p.widthPt * s).toDouble(), (p.heightPt * s).toDouble(), lx, ly) }) { r ->
            if (d === doc) then(r.getOrDefault(-1))
        }
    }

    /** Long press: selects the word under the finger; moving then extends it. */
    private fun startSelection(x: Float, y: Float) {
        val d = doc ?: return
        val slot = slotAt(x, y) ?: return
        val page = slot.page
        charAt(page, x, y) { c ->
            if (c < 0) return@charAt
            PdfEngine.run({ d.textWordAt(page.toUInt(), c) }) { r ->
                val w = r.getOrNull() ?: return@run
                if (d !== doc || w.count <= 0) return@run
                clearSelection()
                selection = Selection(page, w.start, w.start + w.count - 1)
                showHandles = true
                requestSelectionRects()
                performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
                if (fingerDown) {
                    // The same gesture keeps going: moving the finger extends it.
                    dragging = Handle.END
                    longPressWord = w.start..(w.start + w.count - 1)
                    longPressPoint = PointF(x, y)
                    dragDx = 0f
                    dragDy = 0f
                } else {
                    showActionMode()
                }
            }
        }
    }

    private fun beginMouseSelection(down: PointF, x: Float, y: Float) {
        val slot = slotAt(down.x, down.y) ?: return
        charAt(slot.page, down.x, down.y) { c ->
            if (c < 0 || !mouseSelecting) return@charAt
            selection = Selection(slot.page, c, c)
            showHandles = false
            requestSelectionRects()
            dragTo(x, y)
        }
    }

    /** Moves the selection's free end to the character at (x, y). */
    private fun dragTo(x: Float, y: Float) {
        val sel = selection ?: return
        if (charQueryBusy) {
            pendingDrag = PointF(x, y)
            return
        }
        charQueryBusy = true
        charAt(sel.page, x, y) { c ->
            charQueryBusy = false
            if (sel === selection && c >= 0) {
                val w = longPressWord
                // After a long press the selection always includes the word.
                val (anchor, focus) = when {
                    w == null -> sel.anchor to c
                    c >= w.first -> w.first to max(c, w.last)
                    else -> w.last to c
                }
                if (anchor != sel.anchor || focus != sel.focus) {
                    sel.anchor = anchor
                    sel.focus = focus
                    requestSelectionRects()
                }
            }
            pendingDrag?.let {
                pendingDrag = null
                dragTo(it.x, it.y)
            }
        }
    }

    private fun requestSelectionRects() {
        val d = doc ?: return
        val sel = selection ?: return
        val p = pages.getOrNull(sel.page) ?: return
        val s = scale
        val token = ++selectionToken
        PdfEngine.run({
            d.textRects(sel.page.toUInt(), (p.widthPt * s).toDouble(), (p.heightPt * s).toDouble(), sel.start, sel.end - sel.start + 1)
        }) { r ->
            if (token != selectionToken) return@run
            selectionRects = r.getOrNull().orEmpty().map { RectF(it.left, it.top, it.right, it.bottom) }
            selectionScale = s
            actionMode?.invalidateContentRect()
            invalidate()
        }
    }

    /** Selection rectangles on the view. */
    private fun selectionOnScreen(): List<RectF> {
        val sel = selection ?: return emptyList()
        val slot = slotOf(sel.page) ?: return emptyList()
        val f = scale / selectionScale
        val l = slot.x - offsetX
        val t = slot.y - offsetY
        return selectionRects.map { RectF(l + it.left * f, t + it.top * f, l + it.right * f, t + it.bottom * f) }
    }

    private fun handleBounds(h: Handle, rects: List<RectF>): Rect? {
        val d = (if (h == Handle.START) handleLeft else handleRight) ?: return null
        val r = (if (h == Handle.START) rects.firstOrNull() else rects.lastOrNull()) ?: return null
        val w = d.intrinsicWidth
        val x = if (h == Handle.START) r.left else r.right
        // The hotspot is the top corner next to the text.
        val left = if (h == Handle.START) x - w * 3 / 4f else x - w / 4f
        return Rect(left.roundToInt(), r.bottom.roundToInt(), (left + w).roundToInt(), (r.bottom + d.intrinsicHeight).roundToInt())
    }

    private fun drawSelection(canvas: Canvas) {
        if (selection == null) return
        val rects = selectionOnScreen()
        for (r in rects) canvas.drawRect(r, selectionPaint)
        if (!showHandles) return
        for (h in Handle.entries) {
            val d = (if (h == Handle.START) handleLeft else handleRight) ?: continue
            val b = handleBounds(h, rects) ?: continue
            d.bounds = b
            d.draw(canvas)
        }
    }

    private fun handleAt(x: Float, y: Float): Handle? {
        if (selection == null || !showHandles) return null
        val rects = selectionOnScreen()
        val slop = 8 * density
        return Handle.entries.firstOrNull { h ->
            val b = handleBounds(h, rects) ?: return@firstOrNull false
            x >= b.left - slop && x <= b.right + slop && y >= b.top - slop && y <= b.bottom + slop
        }
    }

    private fun beginHandleDrag(h: Handle, x: Float, y: Float) {
        val sel = selection ?: return
        val rects = selectionOnScreen()
        val line = (if (h == Handle.START) rects.firstOrNull() else rects.lastOrNull()) ?: return
        // The other end stays; the finger moves this one.
        sel.anchor = if (h == Handle.START) sel.end else sel.start
        sel.focus = if (h == Handle.START) sel.start else sel.end
        dragging = h
        longPressWord = null
        dragDx = x - (if (h == Handle.START) line.left else line.right)
        dragDy = y - line.centerY()
        actionMode?.finish()
        parent?.requestDisallowInterceptTouchEvent(true)
    }

    private fun onHandleDrag(e: MotionEvent) {
        when (e.actionMasked) {
            MotionEvent.ACTION_MOVE -> {
                // A finger held still after the long press does not move the end.
                if (longPressWord != null && hypot(e.x - longPressPoint.x, e.y - longPressPoint.y) < touchSlop) return
                actionMode?.finish()
                dragTo(e.x - dragDx, e.y - dragDy)
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                dragging = null
                longPressWord = null
                showActionMode()
            }
        }
    }

    private fun selectAllOnPage() {
        val d = doc ?: return
        val sel = selection ?: return
        PdfEngine.run({ d.textCharCount(sel.page.toUInt()) }) { r ->
            val n = r.getOrDefault(0)
            if (sel !== selection || n <= 0) return@run
            sel.anchor = 0
            sel.focus = n - 1
            requestSelectionRects()
        }
    }

    /** Copies the selected text. False if nothing is selected. */
    fun copySelection(): Boolean {
        val d = doc ?: return false
        val sel = selection ?: return false
        PdfEngine.run({ d.textRange(sel.page.toUInt(), sel.start, sel.end - sel.start + 1) }) { r ->
            val text = r.getOrNull().orEmpty()
            if (text.isEmpty()) return@run
            val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            cm.setPrimaryClip(ClipData.newPlainText(null, text))
            // Android 13+ confirms the copy itself.
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
                Toast.makeText(context, R.string.copied, Toast.LENGTH_SHORT).show()
            }
        }
        return true
    }

    private fun showActionMode() {
        if (selection == null) return
        actionMode?.let {
            it.invalidateContentRect()
            return
        }
        actionMode = startActionMode(object : ActionMode.Callback2() {
            override fun onCreateActionMode(mode: ActionMode, menu: Menu): Boolean {
                menu.add(Menu.NONE, MENU_COPY, 0, android.R.string.copy)
                menu.add(Menu.NONE, MENU_SELECT_ALL, 1, android.R.string.selectAll)
                return true
            }

            override fun onPrepareActionMode(mode: ActionMode, menu: Menu) = false

            override fun onActionItemClicked(mode: ActionMode, item: MenuItem): Boolean {
                when (item.itemId) {
                    MENU_COPY -> {
                        copySelection()
                        clearSelection()
                    }
                    MENU_SELECT_ALL -> selectAllOnPage()
                }
                return true
            }

            override fun onDestroyActionMode(mode: ActionMode) {
                if (actionMode === mode) actionMode = null
            }

            override fun onGetContentRect(mode: ActionMode, view: View, outRect: Rect) {
                val rects = selectionOnScreen()
                if (rects.isEmpty()) return super.onGetContentRect(mode, view, outRect)
                val u = RectF(rects[0])
                for (r in rects) u.union(r)
                u.intersect(0f, 0f, width.toFloat(), height.toFloat())
                u.roundOut(outRect)
            }
        }, ActionMode.TYPE_FLOATING)
    }

    // -----------------------------------------------------------------------
    // Form input
    // -----------------------------------------------------------------------

    private fun tap(x: Float, y: Float) {
        val d = doc ?: return
        val slot = slotAt(x, y) ?: return
        val i = slot.page
        val p = pages[i]
        val s = scale.toDouble()
        val pageW = p.widthPt * s
        val pageH = p.heightPt * s
        val lx = (x - (slot.x - offsetX)).toDouble()
        val ly = (y - (slot.y - offsetY)).toDouble()
        requestFocus()
        commitComposing()
        send { d.tap(i.toUInt(), pageW, pageH, lx, ly) }
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
        when (keyCode) {
            KeyEvent.KEYCODE_PAGE_DOWN -> {
                scrollByScreen(1)
                return true
            }
            KeyEvent.KEYCODE_PAGE_UP -> {
                scrollByScreen(-1)
                return true
            }
            KeyEvent.KEYCODE_ESCAPE -> if (selection != null) {
                clearSelection()
                return true
            }
        }
        // Shortcuts belong to the activity.
        if (event.isCtrlPressed || event.isAltPressed || event.isMetaPressed) return super.onKeyDown(keyCode, event)
        val key = when (keyCode) {
            KeyEvent.KEYCODE_DEL -> Key.BACKSPACE
            KeyEvent.KEYCODE_FORWARD_DEL -> Key.DELETE
            KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> Key.ENTER
            KeyEvent.KEYCODE_TAB -> Key.TAB
            KeyEvent.KEYCODE_ESCAPE -> Key.ESCAPE
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

    companion object {
        private const val TILE_W = 1024
        private const val TILE_H = 512
        private const val MENU_COPY = 1
        private const val MENU_SELECT_ALL = 2

        /** Zoom levels of the zoom in/out buttons, in percent. */
        val ZOOM_STEPS = intArrayOf(10, 25, 33, 50, 67, 75, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500, 600, 800)
    }
}
