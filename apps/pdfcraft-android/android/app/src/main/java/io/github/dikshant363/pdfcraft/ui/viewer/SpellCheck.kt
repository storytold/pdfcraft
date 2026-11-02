package io.github.dikshant363.pdfcraft.ui.viewer

import android.content.Context
import android.view.textservice.SentenceSuggestionsInfo
import android.view.textservice.SpellCheckerSession
import android.view.textservice.SuggestionsInfo
import android.view.textservice.TextInfo
import android.view.textservice.TextServicesManager
import java.util.Locale
import kotlin.coroutines.resume
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withTimeoutOrNull

/** A word the spell checker doesn't know: characters [start] until [end], and what it suggests. */
data class Typo(val start: Int, val end: Int, val suggestions: List<String>)

/**
 * Spell checking for Edit PDF's text, as the reference underlines misspelled words while editing:
 * the phone's own spell checker (the keyboard's), asked word by word (the request every checker
 * answers; some give nothing useful for whole sentences). Nothing is reported when the phone has
 * no spell checker or it doesn't answer.
 */
class SpellCheck(context: Context) {
    private val manager = context.getSystemService(TextServicesManager::class.java)

    suspend fun check(text: String): List<Typo> {
        if (text.isBlank()) return emptyList()
        val tsm = manager ?: return emptyList()
        // the words, with where each starts (letters and apostrophes; at most 200 of them)
        val words = Regex("[\\p{L}][\\p{L}'’]*").findAll(text).take(200).toList()
        if (words.isEmpty()) return emptyList()
        return withTimeoutOrNull(3000) {
            suspendCancellableCoroutine { cont ->
                var session: SpellCheckerSession? = null
                val listener = object : SpellCheckerSession.SpellCheckerSessionListener {
                    override fun onGetSuggestions(results: Array<out SuggestionsInfo>?) {
                        val typos = mutableListOf<Typo>()
                        results.orEmpty().forEachIndexed { i, info ->
                            val m = words.getOrNull(i) ?: return@forEachIndexed
                            val attrs = info.suggestionsAttributes
                            if (attrs and SuggestionsInfo.RESULT_ATTR_LOOKS_LIKE_TYPO == 0) return@forEachIndexed
                            if (attrs and SuggestionsInfo.RESULT_ATTR_IN_THE_DICTIONARY != 0) return@forEachIndexed
                            val suggestions = (0 until info.suggestionsCount.coerceAtLeast(0)).mapNotNull { info.getSuggestionAt(it) }.filter { it.isNotBlank() }
                            if (suggestions.any { it.equals(m.value, ignoreCase = true) }) return@forEachIndexed
                            typos += Typo(m.range.first, m.range.last + 1, suggestions.take(3))
                        }
                        session?.close()
                        if (cont.isActive) cont.resume(typos)
                    }

                    override fun onGetSentenceSuggestions(results: Array<out SentenceSuggestionsInfo>?) {}
                }
                session = runCatching { tsm.newSpellCheckerSession(null, Locale.getDefault(), listener, true) }.getOrNull()
                if (session == null) {
                    cont.resume(emptyList())
                } else {
                    cont.invokeOnCancellation { session?.close() }
                    @Suppress("DEPRECATION")
                    session.getSuggestions(words.map { TextInfo(it.value) }.toTypedArray(), 3, true)
                }
            }
        } ?: emptyList()
    }
}
