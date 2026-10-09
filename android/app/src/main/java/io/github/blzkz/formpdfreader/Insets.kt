package io.github.blzkz.formpdfreader

import android.view.View
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

/**
 * The app draws behind the status and navigation bars (as Android 15
 * requires). [root] is padded so its content stays clear of the bars, the
 * display cutout and the keyboard; when [bottomBar] is given, it extends
 * under the navigation bar instead and hides while the keyboard is open.
 */
fun applySystemInsets(root: View, bottomBar: View? = null) {
    ViewCompat.setOnApplyWindowInsetsListener(root) { v, insets ->
        val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
        val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
        val keyboard = insets.isVisible(WindowInsetsCompat.Type.ime())
        if (bottomBar != null) {
            bottomBar.visibility = if (keyboard) View.GONE else View.VISIBLE
            bottomBar.setPadding(0, 0, 0, bars.bottom)
            v.setPadding(bars.left, bars.top, bars.right, if (keyboard) ime.bottom else 0)
        } else {
            v.setPadding(bars.left, bars.top, bars.right, maxOf(bars.bottom, ime.bottom))
        }
        WindowInsetsCompat.CONSUMED
    }
}
