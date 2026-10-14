// Accuracy fixture for the heuristic Kotlin extractor. Read as text, never compiled.
package uploads

class Uploader(val path: String) {
    fun upload(): Boolean {
        return path.isNotEmpty()
    }
}

data class UploadResult(val ok: Boolean)

fun uploadRetry(attempts: Int): Boolean = attempts < 3

private fun helper() = Unit

/*
fun blockCommented() {}
*/

val template = """
fun insideRawString() {}
"""
