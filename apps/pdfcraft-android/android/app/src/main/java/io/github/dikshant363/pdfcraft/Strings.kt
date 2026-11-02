package io.github.dikshant363.pdfcraft

import android.content.Context
import androidx.annotation.PluralsRes
import androidx.annotation.StringRes

/**
 * The app's text lives in `res/values/strings.xml` (a translation is a `values-<language>` copy
 * of it). Composables read it with `stringResource`; code with no Context of its own (the view
 * model, the engine's error messages) reads it here, in the language the app runs in.
 */
private lateinit var appContext: Context

internal fun initText(context: Context) {
    appContext = context.applicationContext
}

/** The string [id], formatted with [args]. */
fun text(@StringRes id: Int, vararg args: Any): String = appContext.getString(id, *args)

/** The plural string [id] for [count], formatted with [args]. */
fun plural(@PluralsRes id: Int, count: Int, vararg args: Any): String = appContext.resources.getQuantityString(id, count, *args)
