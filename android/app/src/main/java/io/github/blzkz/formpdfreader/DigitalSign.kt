package io.github.blzkz.formpdfreader

import android.content.Context
import android.text.InputType
import android.view.ViewGroup
import android.widget.ArrayAdapter
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.RadioButton
import android.widget.RadioGroup
import android.widget.ScrollView
import android.widget.Spinner
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import uniffi.form_pdf_reader_ffi.SignatureFieldInfo
import uniffi.form_pdf_reader_ffi.SignatureInfo

/**
 * Digital signature with a certificate: the options dialog (which
 * certificate, visible or not and where, the handwritten signature next to
 * it, reason and location) and the password of a certificate file.
 * [MainActivity] does the rest: choosing the certificate, the place on the
 * page, signing and saving the signed copy.
 */
object DigitalSign {
    enum class Source { DEVICE, FILE }

    class Options(
        val source: Source,
        val visible: Boolean,
        /** A signature field of the document; null: tap on the page. */
        val field: String?,
        val handwrittenId: String?,
        val reason: String,
        val location: String,
    )

    private fun dp(c: Context, v: Float) = (v * c.resources.displayMetrics.density).toInt()

    private fun label(c: Context, text: Int, top: Float = 12f) = TextView(c).apply {
        setText(text)
        setTextAppearance(com.google.android.material.R.style.TextAppearance_Material3_LabelLarge)
        setPadding(0, dp(c, top), 0, dp(c, 4f))
    }

    fun showOptions(
        a: AppCompatActivity,
        fields: List<SignatureFieldInfo>,
        saved: List<SignatureInfo>,
        dynamicXfa: Boolean,
        onDone: (Options) -> Unit,
    ) {
        val box = LinearLayout(a).apply { orientation = LinearLayout.VERTICAL; setPadding(dp(a, 24f), dp(a, 4f), dp(a, 24f), 0) }
        fun radio(text: Int, id: Int) = RadioButton(a).apply { setText(text); this.id = id }

        box.addView(label(a, R.string.dsig_certificate, 0f))
        val source = RadioGroup(a).apply {
            addView(radio(R.string.dsig_device, 1))
            addView(radio(R.string.dsig_file, 2))
            check(1)
        }
        box.addView(source)

        box.addView(label(a, R.string.dsig_appearance))
        val look = RadioGroup(a).apply {
            orientation = RadioGroup.HORIZONTAL
            addView(radio(R.string.dsig_visible, 1))
            addView(radio(R.string.dsig_invisible, 2))
            check(if (dynamicXfa) 2 else 1)
        }
        if (dynamicXfa) for (i in 0 until look.childCount) look.getChildAt(i).isEnabled = false
        box.addView(look)

        // Where (empty signature fields of the document, or a tap on the page).
        val empty = fields.filter { !it.signed }
        val where = Spinner(a)
        val whereItems = listOf(a.getString(R.string.dsig_where_tap)) + empty.map { f ->
            f.page?.let { a.getString(R.string.dsig_field_page, f.name, (it + 1u).toString()) } ?: a.getString(R.string.dsig_field, f.name)
        }
        where.adapter = ArrayAdapter(a, android.R.layout.simple_spinner_dropdown_item, whereItems)
        if (empty.isNotEmpty()) where.setSelection(1)
        val whereLabel = label(a, R.string.dsig_where)

        // The handwritten signature next to the text.
        val hand = Spinner(a)
        val handItems = listOf(a.getString(R.string.dsig_handwritten_none)) + saved.mapIndexed { i, s ->
            a.getString(if (s.initials) R.string.dsig_saved_initials else R.string.dsig_saved_signature, i + 1)
        }
        hand.adapter = ArrayAdapter(a, android.R.layout.simple_spinner_dropdown_item, handItems)
        val handLabel = label(a, R.string.dsig_handwritten)
        val visibleViews = listOf(whereLabel, where, handLabel, hand)
        for (v in visibleViews) box.addView(v)
        fun updateVisible() {
            val on = look.checkedRadioButtonId == 1
            for (v in visibleViews) v.visibility = if (on) android.view.View.VISIBLE else android.view.View.GONE
        }
        look.setOnCheckedChangeListener { _, _ -> updateVisible() }
        updateVisible()

        fun field(hint: Int) = EditText(a).apply {
            setHint(hint)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_SENTENCES
            isSingleLine = true
        }
        box.addView(label(a, R.string.dsig_reason))
        val reason = field(R.string.dsig_optional)
        box.addView(reason)
        box.addView(label(a, R.string.dsig_location))
        val location = field(R.string.dsig_optional)
        box.addView(location)

        val notes = buildList {
            if (dynamicXfa) add(a.getString(R.string.dsig_note_xfa))
            add(a.getString(R.string.dsig_note_copy))
        }
        box.addView(TextView(a).apply {
            text = notes.joinToString("\n\n")
            setTextAppearance(com.google.android.material.R.style.TextAppearance_Material3_BodySmall)
            setPadding(0, dp(a, 12f), 0, 0)
        })

        MaterialAlertDialogBuilder(a)
            .setTitle(R.string.dsig_title)
            .setView(ScrollView(a).apply { addView(box, ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)) })
            .setPositiveButton(R.string.dsig_continue) { _, _ ->
                val visible = look.checkedRadioButtonId == 1
                val w = where.selectedItemPosition
                val h = hand.selectedItemPosition
                onDone(
                    Options(
                        source = if (source.checkedRadioButtonId == 2) Source.FILE else Source.DEVICE,
                        visible = visible,
                        field = if (visible && w > 0) empty[w - 1].name else null,
                        handwrittenId = if (visible && h > 0) saved[h - 1].id else null,
                        reason = reason.text.toString().trim(),
                        location = location.text.toString().trim(),
                    ),
                )
            }
            .setNegativeButton(R.string.cancel, null)
            .show()
    }

    /** Asks the password of the certificate file [name]. */
    fun askPassword(a: AppCompatActivity, name: String, wrong: Boolean, onPassword: (String) -> Unit) {
        val input = EditText(a).apply {
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            isSingleLine = true
        }
        val box = LinearLayout(a).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(a, 24f), dp(a, 8f), dp(a, 24f), 0)
            if (wrong) addView(TextView(a).apply { setText(R.string.dsig_password_wrong) })
            addView(input)
        }
        MaterialAlertDialogBuilder(a)
            .setTitle(a.getString(R.string.dsig_password_title, name))
            .setView(box)
            .setPositiveButton(R.string.ok) { _, _ -> onPassword(input.text.toString()) }
            .setNegativeButton(R.string.cancel, null)
            .show()
        input.requestFocus()
    }
}
