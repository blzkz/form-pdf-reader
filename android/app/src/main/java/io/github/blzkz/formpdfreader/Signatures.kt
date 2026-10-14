package io.github.blzkz.formpdfreader

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.drawable.GradientDrawable
import android.text.Editable
import android.text.TextWatcher
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import com.google.android.material.button.MaterialButton
import com.google.android.material.button.MaterialButtonToggleGroup
import com.google.android.material.color.MaterialColors
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.tabs.TabLayout
import com.google.android.material.textfield.TextInputEditText
import com.google.android.material.textfield.TextInputLayout
import uniffi.form_pdf_reader_ffi.DigitalSignatureInfo
import uniffi.form_pdf_reader_ffi.InkStroke
import uniffi.form_pdf_reader_ffi.SignatureState
import uniffi.form_pdf_reader_ffi.SignatureImage
import uniffi.form_pdf_reader_ffi.SignatureLibrary
import uniffi.form_pdf_reader_ffi.nameInitials
import uniffi.form_pdf_reader_ffi.previewTyped
import uniffi.form_pdf_reader_ffi.signatureStyles
import java.io.File
import java.nio.ByteBuffer

/**
 * Handwritten signatures, as in DocuSign: the name or the initials in
 * handwriting, or a signature drawn with the finger or a stylus. They are
 * kept on the device (in the app's own folder) to use them again.
 */
object Signatures {
    /** Ink colours: dark blue (the default) and black. */
    const val INK_BLUE = 0x1B2A4A
    const val INK_BLACK = 0x141414

    private var lib: SignatureLibrary? = null

    fun library(c: Context): SignatureLibrary =
        lib ?: SignatureLibrary(File(c.filesDir, "signatures").absolutePath).also { lib = it }

    private fun prefs(c: Context) = c.getSharedPreferences("signatures", Context.MODE_PRIVATE)

    /** The name used for the last typed signature (proposed for the next one). */
    private fun lastName(c: Context) = prefs(c).getString("name", "") ?: ""

    /** A signature image as a bitmap (Android wants the colour premultiplied by the alpha). */
    fun bitmapOf(img: SignatureImage): Bitmap {
        val w = img.width.toInt()
        val h = img.height.toInt()
        val px = img.rgba.copyOf()
        for (i in 0 until w * h) {
            val a = px[i * 4 + 3].toInt() and 0xFF
            if (a == 255) continue
            for (k in 0..2) px[i * 4 + k] = ((px[i * 4 + k].toInt() and 0xFF) * a / 255).toByte()
        }
        return Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888).apply { copyPixelsFromBuffer(ByteBuffer.wrap(px)) }
    }

    private fun dp(c: Context, v: Float) = (v * c.resources.displayMetrics.density).toInt()

    /** The saved signatures to choose one; [onPlace] gets its id. */
    /**
     * The signing menu: the saved signatures to place one, create one, or
     * sign with a certificate ([onDigital]). [handwritten]: the document
     * accepts signatures on the page (not dynamic XFA forms).
     */
    fun showMenu(a: AppCompatActivity, handwritten: Boolean, onPlace: (String) -> Unit, onDigital: () -> Unit) {
        val library = library(a)
        val list = LinearLayout(a).apply { orientation = LinearLayout.VERTICAL; setPadding(dp(a, 20f), dp(a, 8f), dp(a, 20f), 0) }
        val builder = MaterialAlertDialogBuilder(a)
            .setTitle(R.string.sign)
            .setView(ScrollView(a).apply { addView(list) })
            .setNeutralButton(R.string.dsig_menu) { _, _ -> onDigital() }
            .setNegativeButton(R.string.cancel, null)
        if (handwritten) builder.setPositiveButton(R.string.sign_create) { _, _ -> showCreate(a) { id -> onPlace(id) } }
        val dialog = builder.create()
        fun fill() {
            list.removeAllViews()
            if (!handwritten) {
                list.addView(TextView(a).apply { setText(R.string.sign_cannot) })
                return
            }
            val items = library.list()
            if (items.isEmpty()) {
                list.addView(TextView(a).apply { setText(R.string.sign_none) })
            }
            for (s in items) {
                val row = LinearLayout(a).apply { orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL }
                val img = ImageView(a).apply {
                    adjustViewBounds = true
                    scaleType = ImageView.ScaleType.FIT_START
                    contentDescription = a.getString(R.string.sign)
                    background = card(a, selected = false)
                    setPadding(dp(a, 12f), dp(a, 8f), dp(a, 12f), dp(a, 8f))
                    setOnClickListener {
                        dialog.dismiss()
                        onPlace(s.id)
                    }
                }
                row.addView(img, LinearLayout.LayoutParams(0, dp(a, 64f), 1f))
                val delete = ImageButton(a).apply {
                    setImageResource(R.drawable.ic_close)
                    contentDescription = a.getString(R.string.sign_delete)
                    setBackgroundResource(android.R.color.transparent)
                    setOnClickListener {
                        library.remove(s.id)
                        fill()
                    }
                }
                row.addView(delete, LinearLayout.LayoutParams(dp(a, 48f), dp(a, 48f)))
                list.addView(row, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { bottomMargin = dp(a, 8f) })
                PdfEngine.run({ library.preview(s.id, dp(a, 48f).toUInt()) }) { r ->
                    r.getOrNull()?.let { img.setImageBitmap(bitmapOf(it)) }
                }
            }
        }
        fill()
        dialog.show()
    }

    /** The digital signatures of a document (see `PdfDocument.digitalSignatures`). */
    fun showDigital(a: AppCompatActivity, list: List<DigitalSignatureInfo>, usageRights: Boolean) {
        val root = LinearLayout(a).apply { orientation = LinearLayout.VERTICAL; setPadding(dp(a, 24f), dp(a, 8f), dp(a, 24f), 0) }
        val weak = MaterialColors.getColor(a, com.google.android.material.R.attr.colorOnSurfaceVariant, Color.GRAY)
        fun line(text: String, color: Int? = null, size: Float = 14f, bold: Boolean = false, top: Float = 2f) {
            root.addView(TextView(a).apply {
                this.text = text
                textSize = size
                color?.let { setTextColor(it) }
                if (bold) setTypeface(typeface, android.graphics.Typeface.BOLD)
                setPadding(0, dp(a, top), 0, 0)
            })
        }
        if (list.isEmpty()) line(a.getString(R.string.digital_none), weak)
        for (s in list) {
            line(s.signer ?: a.getString(R.string.digital_unknown_signer), size = 17f, bold = true, top = 12f)
            val (mark, color, text) = when (s.state) {
                SignatureState.INTACT -> Triple("✓", Color.rgb(0x2E, 0x7D, 0x32), R.string.digital_intact)
                SignatureState.CHANGED_AFTER -> Triple("⚠", Color.rgb(0xC7, 0x7C, 0x00), R.string.digital_changed_after)
                SignatureState.BROKEN -> Triple("✕", Color.rgb(0xC6, 0x28, 0x28), R.string.digital_broken)
                SignatureState.UNKNOWN -> Triple("?", weak, R.string.digital_unknown)
            }
            line("$mark  ${a.getString(text)}", color)
            val kind = when (s.certification?.toInt()) {
                null -> R.string.digital_approval
                1 -> R.string.digital_cert_none
                2 -> R.string.digital_cert_fill
                else -> R.string.digital_cert_annots
            }
            line("${a.getString(R.string.digital_kind)} ${a.getString(kind)}", top = 6f)
            s.time?.let { line("${a.getString(R.string.digital_date)} $it") }
            s.reason?.let { line("${a.getString(R.string.digital_reason)} $it") }
            s.issuer?.let { line("${a.getString(R.string.digital_issuer)} $it") }
            val from = s.validFrom
            val to = s.validTo
            if (from != null && to != null) line("${a.getString(R.string.digital_validity)} $from – $to")
        }
        if (usageRights) line(a.getString(R.string.digital_usage_rights), weak, top = 14f)
        if (list.isNotEmpty()) line(a.getString(R.string.digital_trust_note), weak, size = 12f, top = 14f)
        MaterialAlertDialogBuilder(a)
            .setTitle(R.string.digital_title)
            .setView(ScrollView(a).apply { addView(root) })
            .setPositiveButton(R.string.ok, null)
            .show()
    }

    /** White card with a border (the accent colour when [selected]). */
    private fun card(c: Context, selected: Boolean) = GradientDrawable().apply {
        setColor(Color.WHITE)
        cornerRadius = 8 * c.resources.displayMetrics.density
        val accent = MaterialColors.getColor(c, androidx.appcompat.R.attr.colorPrimary, Color.BLUE)
        setStroke(dp(c, if (selected) 2f else 1f), if (selected) accent else Color.rgb(0xD5, 0xD9, 0xDE))
    }

    /** Creates a signature: typed name, initials or drawn. [onCreated] gets its id. */
    fun showCreate(a: AppCompatActivity, onCreated: (String) -> Unit) {
        val library = library(a)
        val styles = signatureStyles()
        var mode = 0
        var style = styles.first()
        var color = INK_BLUE

        val root = LinearLayout(a).apply { orientation = LinearLayout.VERTICAL; setPadding(dp(a, 20f), dp(a, 4f), dp(a, 20f), 0) }
        val tabs = TabLayout(a)
        for (t in intArrayOf(R.string.sign_mode_typed, R.string.sign_mode_initials, R.string.sign_mode_drawn)) tabs.addTab(tabs.newTab().setText(t))
        root.addView(tabs)

        // Typed name and initials.
        val typed = LinearLayout(a).apply { orientation = LinearLayout.VERTICAL }
        val nameLayout = TextInputLayout(a).apply { hint = a.getString(R.string.sign_name) }
        val name = TextInputEditText(nameLayout.context).apply { setSingleLine(); setText(lastName(a)) }
        nameLayout.addView(name)
        typed.addView(nameLayout, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = dp(a, 12f) })
        val initialsLayout = TextInputLayout(a).apply { hint = a.getString(R.string.sign_initials) }
        val initials = TextInputEditText(initialsLayout.context).apply { setSingleLine(); setText(nameInitials(name.text.toString())) }
        initialsLayout.addView(initials)
        typed.addView(initialsLayout, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = dp(a, 12f) })
        typed.addView(TextView(a).apply { setText(R.string.sign_style); setPadding(0, dp(a, 12f), 0, dp(a, 4f)) })
        val styleViews = styles.map { s ->
            ImageView(a).apply {
                scaleType = ImageView.ScaleType.FIT_CENTER
                setPadding(dp(a, 12f), dp(a, 6f), dp(a, 12f), dp(a, 6f))
                contentDescription = s
            }
        }
        for (v in styleViews) typed.addView(v, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(a, 60f)).apply { bottomMargin = dp(a, 8f) })

        // Drawn.
        val drawn = LinearLayout(a).apply { orientation = LinearLayout.VERTICAL; visibility = View.GONE }
        drawn.addView(TextView(a).apply { setText(R.string.sign_draw_hint); setPadding(0, dp(a, 12f), 0, dp(a, 4f)) })
        val pad = SignaturePad(a)
        drawn.addView(pad, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(a, 190f)))
        drawn.addView(MaterialButton(a, null, androidx.appcompat.R.attr.borderlessButtonStyle).apply {
            setText(R.string.sign_clear)
            setOnClickListener { pad.clear() }
        })

        root.addView(FrameLayout(a).apply { addView(typed); addView(drawn) })

        // Ink.
        val inks = MaterialButtonToggleGroup(a).apply { isSingleSelection = true; isSelectionRequired = true }
        for ((i, pair) in listOf(R.string.sign_ink_blue to INK_BLUE, R.string.sign_ink_black to INK_BLACK).withIndex()) {
            val b = MaterialButton(a, null, com.google.android.material.R.attr.materialButtonOutlinedStyle).apply {
                id = View.generateViewId()
                setText(pair.first)
                // Un punto del color de la tinta (el texto, del tema: se lee en claro y oscuro).
                icon = GradientDrawable().apply {
                    shape = GradientDrawable.OVAL
                    setColor(Color.rgb(pair.second shr 16 and 0xFF, pair.second shr 8 and 0xFF, pair.second and 0xFF))
                    setStroke(dp(a, 1f), Color.WHITE)
                    setSize(dp(a, 16f), dp(a, 16f))
                }
                iconTint = null
                tag = pair.second
            }
            inks.addView(b)
            if (i == 0) inks.check(b.id)
        }
        root.addView(inks, LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT).apply { topMargin = dp(a, 8f) })

        fun text() = (if (mode == 1) initials.text else name.text).toString().trim()
        fun refreshStyles() {
            for ((i, v) in styleViews.withIndex()) {
                v.background = card(a, styles[i] == style)
                val t = text()
                if (t.isEmpty()) {
                    v.setImageDrawable(null)
                    continue
                }
                val s = styles[i]
                val c = color
                val m = mode
                PdfEngine.run({ previewTyped(t, s, m == 1, c.toUInt(), dp(a, 48f).toUInt()) }) { r ->
                    if (t == text() && c == color && m == mode) r.getOrNull()?.let { v.setImageBitmap(bitmapOf(it)) }
                }
            }
        }
        for ((i, v) in styleViews.withIndex()) v.setOnClickListener {
            style = styles[i]
            refreshStyles()
        }
        var initialsEdited = false
        name.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, st: Int, c: Int, after: Int) {}
            override fun onTextChanged(s: CharSequence?, st: Int, b: Int, c: Int) {}
            override fun afterTextChanged(s: Editable?) {
                if (!initialsEdited) initials.setText(nameInitials(s.toString()))
                refreshStyles()
            }
        })
        initials.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, st: Int, c: Int, after: Int) {}
            override fun onTextChanged(s: CharSequence?, st: Int, b: Int, c: Int) {}
            override fun afterTextChanged(s: Editable?) {
                if (initials.hasFocus()) initialsEdited = true
                if (mode == 1) refreshStyles()
            }
        })
        fun showMode() {
            typed.visibility = if (mode == 2) View.GONE else View.VISIBLE
            drawn.visibility = if (mode == 2) View.VISIBLE else View.GONE
            nameLayout.visibility = if (mode == 0) View.VISIBLE else View.GONE
            initialsLayout.visibility = if (mode == 1) View.VISIBLE else View.GONE
            refreshStyles()
        }
        tabs.addOnTabSelectedListener(object : TabLayout.OnTabSelectedListener {
            override fun onTabSelected(t: TabLayout.Tab) {
                mode = t.position
                showMode()
            }
            override fun onTabUnselected(t: TabLayout.Tab) {}
            override fun onTabReselected(t: TabLayout.Tab) {}
        })
        inks.addOnButtonCheckedListener { g, id, checked ->
            if (checked) {
                color = g.findViewById<View>(id).tag as Int
                pad.ink = color
                refreshStyles()
            }
        }
        showMode()

        val dialog = MaterialAlertDialogBuilder(a)
            .setTitle(R.string.sign_create_title)
            .setView(ScrollView(a).apply { addView(root) })
            .setPositiveButton(R.string.sign_save, null)
            .setNegativeButton(R.string.cancel, null)
            .create()
        dialog.setOnShowListener {
            // Checked here so that an empty signature keeps the dialog open.
            dialog.getButton(android.content.DialogInterface.BUTTON_POSITIVE).setOnClickListener {
                val r = runCatching {
                    when (mode) {
                        2 -> {
                            if (pad.isEmpty()) return@setOnClickListener
                            library.addDrawn(pad.strokes(), color.toUInt())
                        }
                        else -> {
                            if (text().isEmpty()) return@setOnClickListener
                            if (mode == 0) prefs(a).edit().putString("name", text()).apply()
                            library.addTyped(text(), style, mode == 1, color.toUInt())
                        }
                    }
                }
                r.onSuccess { id ->
                    dialog.dismiss()
                    onCreated(id)
                }
            }
        }
        dialog.show()
    }
}

/** White pad to draw a signature with the finger or a stylus. */
class SignaturePad(context: Context) : View(context) {
    private val density = resources.displayMetrics.density
    private val lines = mutableListOf<MutableList<Float>>()
    private val penWidth = 2.6f * density
    var ink = Signatures.INK_BLUE
        set(value) {
            field = value
            paint.color = Color.rgb(value shr 16 and 0xFF, value shr 8 and 0xFF, value and 0xFF)
            invalidate()
        }
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = penWidth
        strokeCap = Paint.Cap.ROUND
        strokeJoin = Paint.Join.ROUND
        color = Color.rgb(0x1B, 0x2A, 0x4A)
    }
    private val guide = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = Color.rgb(200, 200, 200); strokeWidth = density }
    private val border = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = Color.rgb(0xD5, 0xD9, 0xDE); style = Paint.Style.STROKE; strokeWidth = density }
    private val fill = Paint().apply { color = Color.WHITE }

    init {
        contentDescription = context.getString(R.string.sign_draw_hint)
    }

    fun clear() {
        lines.clear()
        invalidate()
    }

    fun isEmpty() = lines.none { it.isNotEmpty() }

    fun strokes(): List<InkStroke> = lines.filter { it.isNotEmpty() }.map { InkStroke(penWidth, it.toList()) }

    override fun onTouchEvent(e: MotionEvent): Boolean {
        // Drawing on the pad does not scroll the dialog.
        parent?.requestDisallowInterceptTouchEvent(true)
        when (e.actionMasked) {
            MotionEvent.ACTION_DOWN -> lines += mutableListOf(e.x, e.y)
            MotionEvent.ACTION_MOVE -> {
                val l = lines.lastOrNull() ?: return true
                for (h in 0 until e.historySize) {
                    l += e.getHistoricalX(h)
                    l += e.getHistoricalY(h)
                }
                l += e.x
                l += e.y
            }
        }
        invalidate()
        return true
    }

    override fun onDraw(canvas: Canvas) {
        val r = 8 * density
        canvas.drawRoundRect(0f, 0f, getWidth().toFloat(), height.toFloat(), r, r, fill)
        canvas.drawRoundRect(0f, 0f, getWidth().toFloat(), height.toFloat(), r, r, border)
        val base = height - 44 * density
        canvas.drawLine(24 * density, base, getWidth() - 24 * density, base, guide)
        for (l in lines) {
            if (l.size == 2) {
                canvas.drawPoint(l[0], l[1], paint)
                continue
            }
            val p = Path()
            p.moveTo(l[0], l[1])
            var i = 2
            while (i + 3 < l.size) {
                // Through the midpoints, as the PDF does.
                p.quadTo(l[i], l[i + 1], (l[i] + l[i + 2]) / 2, (l[i + 1] + l[i + 3]) / 2)
                i += 2
            }
            p.lineTo(l[l.size - 2], l[l.size - 1])
            canvas.drawPath(p, paint)
        }
    }
}

