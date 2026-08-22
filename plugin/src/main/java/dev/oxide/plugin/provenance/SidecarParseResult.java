package dev.oxide.plugin.provenance;

/**
 * Result of parsing a sidecar file's bytes. Three-way (OK / ERROR) rather
 * than a boolean + nullable, so a caller cannot accidentally collapse
 * "parse error" into "absent" — the two must be handled differently
 * upstream (absent is silent and normal; a parse error is logged once and
 * must surface as UNKNOWN, never as JAVA).
 */
public final class SidecarParseResult {

    public enum Status { OK, ERROR }

    private final Status status;
    private final byte[] bitmap;
    private final String errorMessage;

    private SidecarParseResult(Status status, byte[] bitmap, String errorMessage) {
        this.status = status;
        this.bitmap = bitmap;
        this.errorMessage = errorMessage;
    }

    public static SidecarParseResult ok(byte[] bitmap) {
        return new SidecarParseResult(Status.OK, bitmap, null);
    }

    public static SidecarParseResult error(String message) {
        return new SidecarParseResult(Status.ERROR, null, message);
    }

    public Status status() {
        return status;
    }

    /** Valid only when status() == OK. 128 bytes, 1024 bits. */
    public byte[] bitmap() {
        return bitmap;
    }

    /** Valid only when status() == ERROR. */
    public String errorMessage() {
        return errorMessage;
    }
}
