// Accuracy fixture for the heuristic Java extractor. Read as text, never compiled.
package uploads;

class Uploader {
    private final String path = "";

    public boolean uploadRetry(int attempts) {
        return attempts < 3;
    }
}

public class UploadService {
    static String describe() {
        return """
            class InsideTextBlock {}
            """;
    }
}

/*
class BlockCommented {}
*/
