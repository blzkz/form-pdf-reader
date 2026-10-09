package io.github.blzkz.formpdfreader

import android.graphics.Typeface
import android.os.Bundle
import android.util.TypedValue
import android.view.ViewGroup
import android.widget.TextView
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.WindowCompat
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.appbar.MaterialToolbar

/**
 * The licence of the app and of the third-party software it includes (Rust
 * crates, PDFium and its components, Android libraries), from the asset
 * written by scripts/android/build-rust.sh.
 */
class LicensesActivity : AppCompatActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        setContentView(R.layout.activity_licenses)
        applySystemInsets(findViewById(R.id.root))
        findViewById<MaterialToolbar>(R.id.toolbar).setNavigationOnClickListener { finish() }

        val list = findViewById<RecyclerView>(R.id.list)
        list.layoutManager = LinearLayoutManager(this)
        Io.run({ assets.open(ASSET).bufferedReader().use { it.readText() } }) { r ->
            // The text in paragraphs: one long text view would be slow.
            val text = r.getOrNull() ?: getString(R.string.licenses_missing)
            list.adapter = Paragraphs(text.split("\n\n").filter { it.isNotBlank() })
        }
    }

    private class Paragraphs(private val items: List<String>) : RecyclerView.Adapter<Paragraphs.Holder>() {
        class Holder(val text: TextView) : RecyclerView.ViewHolder(text)

        override fun getItemCount() = items.size

        override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): Holder {
            val tv = TextView(parent.context).apply {
                layoutParams = RecyclerView.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
                // Monospaced: the notices use plain-text layout.
                typeface = Typeface.MONOSPACE
                setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
                val pad = (6 * resources.displayMetrics.density).toInt()
                setPadding(0, pad, 0, pad)
            }
            return Holder(tv)
        }

        override fun onBindViewHolder(holder: Holder, position: Int) {
            holder.text.text = items[position]
        }
    }

    companion object {
        private const val ASSET = "licenses.txt"
    }
}
