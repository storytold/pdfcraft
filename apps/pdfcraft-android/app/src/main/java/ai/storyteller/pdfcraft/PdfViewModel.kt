package ai.storyteller.pdfcraft

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import uniffi.pdfcraft_mobile_ffi.MobileDocument
import uniffi.pdfcraft_mobile_ffi.MobileException
import uniffi.pdfcraft_mobile_ffi.OutlineEntry
import uniffi.pdfcraft_mobile_ffi.SearchMatch

/** Device pixels per PDF point for both page rendering and search highlight rects — they must
 * match, or a match's rects won't line up with the bitmap they're drawn onto. */
const val RENDER_SCALE = 2.0f

sealed interface PdfState {
    data object Empty : PdfState

    data object Loading : PdfState

    data class Error(val message: String) : PdfState

    data class Loaded(
        val document: MobileDocument,
        val page: Int,
        val pageCount: Int,
        val title: String?,
        val outline: List<OutlineEntry>,
        val matches: List<SearchMatch> = emptyList(),
        val matchIndex: Int = -1,
    ) : PdfState {
        val activeMatch: SearchMatch?
            get() = matches.getOrNull(matchIndex)
    }
}

class PdfViewModel : ViewModel() {
    var state by mutableStateOf<PdfState>(PdfState.Empty)
        private set

    /** `bytes` is null when the picker returned nothing, or the file couldn't be read. */
    fun open(bytes: ByteArray?) {
        (state as? PdfState.Loaded)?.document?.close()
        if (bytes == null) {
            state = PdfState.Error("Could not read the file")
            return
        }
        state = PdfState.Loading
        viewModelScope.launch(Dispatchers.Default) {
            state =
                try {
                    val document = MobileDocument.open(bytes, null)
                    PdfState.Loaded(
                        document,
                        page = 0,
                        pageCount = document.pageCount().toInt(),
                        title = document.title(),
                        outline = document.outline(),
                    )
                } catch (e: MobileException) {
                    PdfState.Error(e.message ?: "This document could not be opened")
                }
        }
    }

    fun goToPage(page: Int) {
        val loaded = state as? PdfState.Loaded ?: return
        if (page in 0 until loaded.pageCount) {
            state = loaded.copy(page = page)
        }
    }

    /** Empty `query` clears the current search instead of running one. */
    fun search(query: String) {
        val loaded = state as? PdfState.Loaded ?: return
        if (query.isBlank()) {
            state = loaded.copy(matches = emptyList(), matchIndex = -1)
            return
        }
        viewModelScope.launch(Dispatchers.Default) {
            val found = loaded.document.search(query, false, false, RENDER_SCALE)
            val current = state as? PdfState.Loaded ?: return@launch
            state =
                if (found.isEmpty()) {
                    current.copy(matches = emptyList(), matchIndex = -1)
                } else {
                    current.copy(matches = found, matchIndex = 0, page = found[0].page.toInt())
                }
        }
    }

    fun nextMatch() = stepMatch(1)

    fun previousMatch() = stepMatch(-1)

    private fun stepMatch(delta: Int) {
        val loaded = state as? PdfState.Loaded ?: return
        if (loaded.matches.isEmpty()) return
        val next = (loaded.matchIndex + delta).mod(loaded.matches.size)
        state = loaded.copy(matchIndex = next, page = loaded.matches[next].page.toInt())
    }

    override fun onCleared() {
        (state as? PdfState.Loaded)?.document?.close()
    }
}
