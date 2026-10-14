package io.github.blzkz.formpdfreader

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.nfc.NfcAdapter
import android.nfc.Tag
import android.nfc.TagLostException
import android.os.Bundle
import android.provider.Settings
import android.text.InputFilter
import android.text.InputType
import android.view.View
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.TextView
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import es.gob.jmulticard.android.nfc.AndroidNfcConnection
import es.gob.jmulticard.callback.CustomTextInputCallback
import es.gob.jmulticard.card.AuthenticationModeLockedException
import es.gob.jmulticard.card.BadPinException
import es.gob.jmulticard.card.dnie.Dnie
import es.gob.jmulticard.card.dnie.DnieFactory
import es.gob.jmulticard.card.dnie.InvalidAccessCodeException
import es.gob.jmulticard.crypto.BcCryptoHelper
import java.security.cert.X509Certificate
import javax.security.auth.callback.CallbackHandler
import javax.security.auth.callback.PasswordCallback
import javax.security.auth.callback.UnsupportedCallbackException

/**
 * The DNIe (Spanish national ID card) over NFC, with jmulticard, the
 * official library: the CAN (the 6 digits on the front of the card) opens
 * the secure channel (PACE) and the PIN unlocks the signing key.
 *
 * Everything happens while the card is held against the phone: reading its
 * signing certificate, preparing the signature of the document and signing.
 */
object DnieReader {
    /** The DNIe's certificate for signing (the other one is for authentication). */
    private const val SIGN_ALIAS = "CertFirmaDigital"

    private fun dp(c: Context, v: Float) = (v * c.resources.displayMetrics.density).toInt()

    private fun prefs(c: Context) = c.getSharedPreferences("dnie", Context.MODE_PRIVATE)

    /** NFC is there and on; otherwise tells the user and returns false. */
    fun checkNfc(a: AppCompatActivity): Boolean {
        val nfc = NfcAdapter.getDefaultAdapter(a)
        if (nfc == null) {
            MaterialAlertDialogBuilder(a).setTitle(R.string.dnie_title).setMessage(R.string.dnie_no_nfc).setPositiveButton(R.string.ok, null).show()
            return false
        }
        if (!nfc.isEnabled) {
            MaterialAlertDialogBuilder(a)
                .setTitle(R.string.dnie_title)
                .setMessage(R.string.dnie_nfc_off)
                .setPositiveButton(R.string.dnie_nfc_settings) { _, _ -> a.startActivity(Intent(Settings.ACTION_NFC_SETTINGS)) }
                .setNegativeButton(R.string.cancel, null)
                .show()
            return false
        }
        return true
    }

    /** Asks the CAN (remembered if the user wants) and the PIN. */
    fun askCodes(a: AppCompatActivity, error: String?, onCodes: (can: String, pin: CharArray) -> Unit) {
        val saved = prefs(a).getString("can", "").orEmpty()
        val box = LinearLayout(a).apply { orientation = LinearLayout.VERTICAL; setPadding(dp(a, 24f), dp(a, 8f), dp(a, 24f), 0) }
        if (error != null) {
            box.addView(TextView(a).apply {
                text = error
                setTextColor(a.getColor(android.R.color.holo_red_light))
                setPadding(0, 0, 0, dp(a, 8f))
            })
        }
        box.addView(TextView(a).apply { setText(R.string.dnie_can_label) })
        val can = EditText(a).apply {
            inputType = InputType.TYPE_CLASS_NUMBER
            filters = arrayOf(InputFilter.LengthFilter(6))
            setText(saved)
        }
        box.addView(can)
        val remember = CheckBox(a).apply {
            setText(R.string.dnie_can_remember)
            isChecked = saved.isNotEmpty()
        }
        box.addView(remember)
        box.addView(TextView(a).apply { setText(R.string.dnie_pin_label); setPadding(0, dp(a, 12f), 0, 0) })
        val pin = EditText(a).apply { inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD }
        box.addView(pin)
        val dialog = MaterialAlertDialogBuilder(a)
            .setTitle(R.string.dnie_title)
            .setView(box)
            .setPositiveButton(R.string.dsig_continue, null)
            .setNegativeButton(R.string.cancel, null)
            .show()
        (if (saved.isEmpty()) can else pin).requestFocus()
        // Checked here so the dialog stays open while the codes are wrong.
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
            val c = can.text.toString().trim()
            val p = pin.text.toString()
            when {
                c.length != 6 -> can.error = a.getString(R.string.dnie_can_invalid)
                p.length < 8 -> pin.error = a.getString(R.string.dnie_pin_invalid)
                else -> {
                    prefs(a).edit().putString("can", if (remember.isChecked) c else "").apply()
                    dialog.dismiss()
                    onCodes(c, p.toCharArray())
                }
            }
        }
    }

    /** What the session does with the card: gets the certificate chain, returns the signed PDF. */
    fun interface Work {
        fun run(chain: List<ByteArray>, sign: (ByteArray) -> ByteArray): ByteArray
    }

    /**
     * Waits for the DNIe and, with it held against the phone, runs [work] in
     * the background. [onDone] gets the signed PDF, or the error to show
     * (null if cancelled) and whether the codes should be asked again.
     */
    fun sign(a: Activity, can: String, pin: CharArray, work: Work, onDone: (Result<ByteArray>?, retryCodes: Boolean) -> Unit) {
        val nfc = NfcAdapter.getDefaultAdapter(a) ?: return onDone(Result.failure(IllegalStateException(a.getString(R.string.dnie_no_nfc))), false)
        val status = TextView(a).apply { setText(R.string.dnie_hold) }
        val progress = ProgressBar(a).apply { isIndeterminate = true }
        val box = LinearLayout(a).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(a, 24f), dp(a, 8f), dp(a, 24f), 0)
            addView(status)
            addView(progress)
        }
        var finished = false
        var busy = false
        lateinit var dialog: AlertDialog
        fun end(r: Result<ByteArray>?, retry: Boolean) {
            if (finished) return
            finished = true
            java.util.Arrays.fill(pin, ' ')
            runCatching { nfc.disableReaderMode(a) }
            if (dialog.isShowing) dialog.dismiss()
            onDone(r, retry)
        }
        dialog = MaterialAlertDialogBuilder(a)
            .setTitle(R.string.dnie_title)
            .setView(box)
            .setNegativeButton(R.string.cancel) { _, _ -> if (!busy) end(null, false) }
            .setCancelable(false)
            .show()
        progress.visibility = View.INVISIBLE
        val flags = NfcAdapter.FLAG_READER_NFC_A or NfcAdapter.FLAG_READER_NFC_B or NfcAdapter.FLAG_READER_SKIP_NDEF_CHECK
        val extras = Bundle().apply { putInt(NfcAdapter.EXTRA_READER_PRESENCE_CHECK_DELAY, 1000) }
        nfc.enableReaderMode(a, { tag: Tag ->
            if (busy || finished) return@enableReaderMode
            busy = true
            a.runOnUiThread {
                status.setText(R.string.dnie_reading)
                progress.visibility = View.VISIBLE
            }
            // On the reader thread: the card has to stay there until the end.
            val r = runCatching { session(tag, can, pin, work) }
            a.runOnUiThread {
                val e = r.exceptionOrNull()
                end(if (e == null) r else Result.failure(friendly(a, e)), retry = e is InvalidAccessCodeException || e is BadPinException)
            }
        }, flags, extras)
    }

    private fun session(tag: Tag, can: String, pin: CharArray, work: Work): ByteArray {
        val conn = AndroidNfcConnection(tag)
        val handler = CallbackHandler { callbacks ->
            for (cb in callbacks) {
                when (cb) {
                    is CustomTextInputCallback -> cb.text = can
                    is PasswordCallback -> cb.password = pin.copyOf()
                    else -> throw UnsupportedCallbackException(cb)
                }
            }
        }
        val dnie: Dnie = DnieFactory.getDnieNfc(conn, BcCryptoHelper(), handler)
        val cert: X509Certificate = dnie.getCertificate(SIGN_ALIAS) ?: error("no signing certificate")
        val key = dnie.getPrivateKey(SIGN_ALIAS)
        return work.run(listOf(cert.encoded)) { data -> dnie.sign(data, "SHA256withRSA", key) }
    }

    /** The error in the user's terms. */
    private fun friendly(c: Context, e: Throwable): Throwable {
        val msg = when {
            e is InvalidAccessCodeException -> c.getString(R.string.dnie_can_wrong)
            e is BadPinException -> c.getString(R.string.dnie_pin_wrong, e.remainingRetries)
            e is AuthenticationModeLockedException -> c.getString(R.string.dnie_pin_locked)
            e is TagLostException || generateSequence(e) { it.cause }.any { it is TagLostException } -> c.getString(R.string.dnie_lost)
            else -> c.getString(R.string.dnie_error, e.message ?: e.javaClass.simpleName)
        }
        return Exception(msg, e)
    }
}
