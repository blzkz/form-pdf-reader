package io.github.blzkz.formpdfreader

import android.graphics.drawable.GradientDrawable
import android.text.InputType
import android.view.View
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.isVisible
import com.google.android.material.button.MaterialButton
import com.google.android.material.button.MaterialButtonToggleGroup
import com.google.android.material.color.MaterialColors
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import io.github.blzkz.formpdfreader.PdfPageView.AnnotRequest
import io.github.blzkz.formpdfreader.PdfPageView.AnnotTool
import uniffi.form_pdf_reader_ffi.AnnotationInfo
import uniffi.form_pdf_reader_ffi.AnnotationKind
import kotlin.math.roundToInt

/**
 * The annotation bar of the viewer: the tool (select, highlight, underline,
 * strike out, note, text, draw), its colour (or the eraser, to remove a mark
 * from the text dragged over), the line width or the text size, and Done.
 * The colours and the size also change the selected annotation (one is
 * selected with Select, or a text with the text tool). It also asks the text
 * of notes and free texts.
 *
 * Marking text and drawing take one finger; two fingers scroll and zoom.
 */
class AnnotationBar(private val activity: AppCompatActivity, private val bar: View, private val onClosed: () -> Unit) {
    private val tools: MaterialButtonToggleGroup = bar.findViewById(R.id.annot_tools)
    private val colors: LinearLayout = bar.findViewById(R.id.annot_colors)
    private val option: MaterialButton = bar.findViewById(R.id.annot_option)
    private val edit: View = bar.findViewById(R.id.annot_edit)
    private val delete: View = bar.findViewById(R.id.annot_delete)
    private val hint: TextView = bar.findViewById(R.id.annot_hint)
    private val density = activity.resources.displayMetrics.density

    /** The viewer being annotated. */
    var view: PdfPageView? = null
        private set

    val isOpen: Boolean get() = view != null

    private var tool = AnnotTool.HIGHLIGHT
    private val colorOf = AnnotTool.entries.associateWith { defaultColor(it) }.toMutableMap()
    private var erasing = false
    private lateinit var eraser: ImageView
    private var inkWidth = 2f
    private var fontSize = 12f

    private val toolIds = mapOf(
        R.id.annot_select to AnnotTool.SELECT,
        R.id.annot_highlight to AnnotTool.HIGHLIGHT,
        R.id.annot_underline to AnnotTool.UNDERLINE,
        R.id.annot_strikeout to AnnotTool.STRIKEOUT,
        R.id.annot_note to AnnotTool.NOTE,
        R.id.annot_text to AnnotTool.TEXT,
        R.id.annot_ink to AnnotTool.INK,
    )

    init {
        tools.addOnButtonCheckedListener { _, id, checked ->
            if (checked) toolIds[id]?.let { setTool(it) }
        }
        bar.findViewById<View>(R.id.annot_done).setOnClickListener { close() }
        edit.setOnClickListener { view?.selectedAnnotation?.let { askText(AnnotRequest.Edit(it)) } }
        delete.setOnClickListener { view?.deleteSelectedAnnotation() }
        option.setOnClickListener {
            val text = editing()?.takeIf { it.kind == AnnotationKind.FREE_TEXT }
            when {
                text != null -> {
                    // The next size up (after being resized by hand it may be none of them).
                    val next = SIZES.firstOrNull { it > text.fontSize } ?: SIZES.first()
                    if (tool == AnnotTool.TEXT) fontSize = next
                    view?.setSelectedFontSize(next)
                }
                tool == AnnotTool.INK -> inkWidth = WIDTHS[(WIDTHS.indexOf(inkWidth) + 1) % WIDTHS.size]
                tool == AnnotTool.TEXT -> fontSize = SIZES[(SIZES.indexOf(fontSize) + 1) % SIZES.size]
            }
            view?.inkWidth = inkWidth
            refresh()
        }
        for (c in COLORS) {
            val chip = View(activity).apply {
                val size = (30 * density).toInt()
                layoutParams = LinearLayout.LayoutParams(size, size).apply { marginEnd = (6 * density).toInt() }
                tag = c
                contentDescription = String.format("#%06X", c)
                setOnClickListener {
                    if (editing()?.kind?.let { it != AnnotationKind.NOTE } == true) view?.setSelectedColor(c)
                    if (tool == AnnotTool.SELECT) return@setOnClickListener
                    colorOf[tool] = c
                    erasing = false
                    view?.annotColor = c
                    view?.annotErasing = false
                    refresh()
                }
            }
            colors.addView(chip)
        }
        eraser = ImageView(activity).apply {
            val size = (30 * density).toInt()
            layoutParams = LinearLayout.LayoutParams(size, size).apply { marginStart = (2 * density).toInt() }
            val pad = (4 * density).toInt()
            setPadding(pad, pad, pad, pad)
            setImageResource(R.drawable.ic_eraser)
            contentDescription = activity.getString(R.string.annot_erase)
            tooltipText = contentDescription
            setOnClickListener {
                erasing = true
                view?.annotErasing = true
                refresh()
            }
        }
        colors.addView(eraser)
    }

    fun open(v: PdfPageView) {
        if (view === v) return
        close()
        view = v
        v.onAnnotRequest = { askText(it) }
        v.onAnnotSelected = { refresh() }
        bar.isVisible = true
        tools.check(toolIds.entries.first { it.value == tool }.key)
        setTool(tool)
    }

    /** Stops annotating (the drawing in progress is written). */
    fun close() {
        val v = view ?: return
        view = null
        v.commitInk()
        v.annotTool = null
        v.onAnnotRequest = null
        v.onAnnotSelected = null
        bar.isVisible = false
        onClosed()
    }

    /** The selected annotation the bar acts on. */
    private fun editing(): AnnotationInfo? = view?.selectedAnnotation?.takeIf {
        tool == AnnotTool.SELECT || (tool == AnnotTool.TEXT && it.kind == AnnotationKind.FREE_TEXT) || (tool == AnnotTool.NOTE && it.kind == AnnotationKind.NOTE)
    }

    private fun setTool(t: AnnotTool) {
        tool = t
        erasing = false
        val v = view ?: return
        v.annotTool = t
        v.annotColor = colorOf.getValue(t)
        v.annotErasing = false
        v.inkWidth = inkWidth
        refresh()
    }

    private fun refresh() {
        val v = view ?: return
        val markup = tool == AnnotTool.HIGHLIGHT || tool == AnnotTool.UNDERLINE || tool == AnnotTool.STRIKEOUT
        val sel = editing()
        val selColor = sel?.takeIf { it.kind != AnnotationKind.NOTE }?.color?.toInt()
        val selText = sel?.takeIf { it.kind == AnnotationKind.FREE_TEXT }
        hint.setText(
            when (tool) {
                AnnotTool.SELECT -> R.string.annot_hint_select
                AnnotTool.HIGHLIGHT, AnnotTool.UNDERLINE, AnnotTool.STRIKEOUT -> if (erasing) R.string.annot_hint_erase else R.string.annot_hint_markup
                AnnotTool.NOTE -> R.string.annot_hint_note
                AnnotTool.TEXT -> R.string.annot_hint_text
                AnnotTool.INK -> R.string.annot_hint_ink
            },
        )
        colors.isVisible = tool != AnnotTool.SELECT || selColor != null
        val current = when {
            selColor != null || tool == AnnotTool.SELECT -> selColor
            erasing -> null
            else -> colorOf.getValue(tool)
        }
        val ring = MaterialColors.getColor(bar, com.google.android.material.R.attr.colorOnSurface)
        for (i in 0 until colors.childCount) {
            val chip = colors.getChildAt(i)
            val c = chip.tag as? Int ?: continue
            chip.background = GradientDrawable().apply {
                shape = GradientDrawable.OVAL
                setColor(0xFF000000.toInt() or c)
                if (c == current) setStroke((3 * density).toInt(), ring)
            }
        }
        eraser.isVisible = markup
        eraser.background = GradientDrawable().apply {
            shape = GradientDrawable.OVAL
            if (erasing) setStroke((3 * density).toInt(), ring)
        }
        option.isVisible = tool == AnnotTool.INK || tool == AnnotTool.TEXT || selText != null
        option.text = when {
            selText != null -> activity.getString(R.string.annot_size, selText.fontSize.roundToInt())
            tool == AnnotTool.INK -> activity.getString(
                when (inkWidth) {
                    1f -> R.string.annot_thin
                    4f -> R.string.annot_thick
                    else -> R.string.annot_medium
                },
            )
            tool == AnnotTool.TEXT -> activity.getString(R.string.annot_size, fontSize.toInt())
            else -> ""
        }
        delete.isVisible = sel != null
        edit.isVisible = sel != null && (sel.kind == AnnotationKind.NOTE || sel.kind == AnnotationKind.FREE_TEXT)
    }

    /** The text of a new note or text, or the new text of one. */
    private fun askText(req: AnnotRequest) {
        val v = view ?: return
        val start = (req as? AnnotRequest.Edit)?.annotation?.contents.orEmpty()
        val input = EditText(activity).apply {
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE or InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
            minLines = 3
            setText(start)
            setSelection(start.length)
        }
        val pad = (20 * density).toInt()
        val box = FrameLayout(activity).apply {
            setPadding(pad, 0, pad, 0)
            addView(input)
        }
        val title = when (req) {
            is AnnotRequest.Note -> R.string.annot_note
            is AnnotRequest.Text -> R.string.annot_text
            is AnnotRequest.Edit -> if (req.annotation.kind == AnnotationKind.NOTE) R.string.annot_edit_note else R.string.annot_edit_text
        }
        MaterialAlertDialogBuilder(activity)
            .setTitle(title)
            .setView(box)
            .setPositiveButton(R.string.ok) { _, _ ->
                val text = input.text.toString()
                when (req) {
                    is AnnotRequest.Note -> v.addNote(req.page, req.x, req.y, text)
                    is AnnotRequest.Text -> if (text.isNotBlank()) v.addFreeText(req.page, req.x, req.y, text, fontSize)
                    is AnnotRequest.Edit -> if (text.isNotBlank() || req.annotation.kind == AnnotationKind.NOTE) v.setAnnotationText(req.annotation, text)
                }
            }
            .setNegativeButton(R.string.cancel, null)
            .show()
        input.requestFocus()
    }

    companion object {
        val COLORS = intArrayOf(0xFFD600, 0x4CAF50, 0x2196F3, 0xE53935, 0x9C27B0, 0x141414)
        private val WIDTHS = listOf(1f, 2f, 4f)
        private val SIZES = listOf(10f, 12f, 14f, 18f, 24f)

        fun defaultColor(t: AnnotTool): Int = when (t) {
            AnnotTool.UNDERLINE -> 0x2196F3
            AnnotTool.STRIKEOUT -> 0xE53935
            AnnotTool.TEXT -> 0x141414
            AnnotTool.INK -> 0x2196F3
            else -> 0xFFD600
        }
    }
}
