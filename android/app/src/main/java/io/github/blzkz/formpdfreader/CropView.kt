package io.github.blzkz.formpdfreader

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.os.Build
import android.view.MotionEvent
import android.view.View
import android.widget.Magnifier
import com.google.android.material.color.MaterialColors
import kotlin.math.hypot
import kotlin.math.min

/**
 * The photo of a scanned page with the four corners of the sheet, which the
 * finger moves (with a magnifier on Android 10+). The corners are kept in
 * fractions of the photo: top left, top right, bottom right, bottom left.
 */
class CropView(context: Context) : View(context) {
    var bitmap: Bitmap? = null
        set(value) {
            field = value
            invalidate()
        }

    /** x0, y0, x1, y1… in fractions of the photo (0 to 1). */
    var corners = floatArrayOf(0.05f, 0.05f, 0.95f, 0.05f, 0.95f, 0.95f, 0.05f, 0.95f)
        set(value) {
            field = value.copyOf()
            invalidate()
        }

    private val density = resources.displayMetrics.density
    private val accent = MaterialColors.getColor(context, androidx.appcompat.R.attr.colorPrimary, Color.rgb(0xF4, 0xB4, 0x00))
    private val linePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 2.5f * density
        color = accent
    }
    private val handlePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = accent }
    private val handleRing = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 2f * density
        color = Color.WHITE
    }
    private val shade = Paint().apply { color = Color.argb(0x88, 0, 0, 0) }
    private val bitmapPaint = Paint(Paint.FILTER_BITMAP_FLAG)
    private var dragging = -1
    private val magnifier: Magnifier? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) Magnifier.Builder(this).build() else null

    /** Where the photo is drawn in the view. */
    private fun frame(): RectF {
        val b = bitmap ?: return RectF()
        val pad = 24 * density
        val s = min((width - 2 * pad) / b.width, (height - 2 * pad) / b.height)
        val w = b.width * s
        val h = b.height * s
        val l = (width - w) / 2
        val t = (height - h) / 2
        return RectF(l, t, l + w, t + h)
    }

    override fun onDraw(canvas: Canvas) {
        val b = bitmap ?: return
        val f = frame()
        canvas.drawBitmap(b, null, f, bitmapPaint)
        val path = Path()
        for (i in 0 until 4) {
            val x = f.left + corners[2 * i] * f.width()
            val y = f.top + corners[2 * i + 1] * f.height()
            if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
        }
        path.close()
        // Dark outside the sheet.
        val outside = Path().apply {
            fillType = Path.FillType.EVEN_ODD
            addRect(f, Path.Direction.CW)
            addPath(path)
        }
        canvas.drawPath(outside, shade)
        canvas.drawPath(path, linePaint)
        for (i in 0 until 4) {
            val x = f.left + corners[2 * i] * f.width()
            val y = f.top + corners[2 * i + 1] * f.height()
            canvas.drawCircle(x, y, 11 * density, handlePaint)
            canvas.drawCircle(x, y, 11 * density, handleRing)
        }
    }

    override fun onTouchEvent(e: MotionEvent): Boolean {
        val f = frame()
        if (f.isEmpty) return false
        when (e.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                var best = -1
                var bestD = 48 * density
                for (i in 0 until 4) {
                    val d = hypot(e.x - (f.left + corners[2 * i] * f.width()), e.y - (f.top + corners[2 * i + 1] * f.height()))
                    if (d < bestD) {
                        bestD = d
                        best = i
                    }
                }
                dragging = best
                if (best >= 0) parent?.requestDisallowInterceptTouchEvent(true)
                return best >= 0
            }
            MotionEvent.ACTION_MOVE -> {
                if (dragging < 0) return false
                corners[2 * dragging] = ((e.x - f.left) / f.width()).coerceIn(0f, 1f)
                corners[2 * dragging + 1] = ((e.y - f.top) / f.height()).coerceIn(0f, 1f)
                magnifier?.show(f.left + corners[2 * dragging] * f.width(), f.top + corners[2 * dragging + 1] * f.height())
                invalidate()
                return true
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                dragging = -1
                magnifier?.dismiss()
                return true
            }
        }
        return false
    }
}
