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

sealed interface PdfState {
    data object Empty : PdfState

    data object Loading : PdfState

    data class Error(val message: String) : PdfState

    data class Loaded(val document: MobileDocument, val page: Int, val pageCount: Int) : PdfState
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
                    PdfState.Loaded(document, page = 0, pageCount = document.pageCount().toInt())
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

    override fun onCleared() {
        (state as? PdfState.Loaded)?.document?.close()
    }
}
