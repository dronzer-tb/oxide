package dev.oxide.plugin.provenance;

/**
 * Result of a chunk provenance lookup. UNKNOWN is distinct from JAVA on
 * purpose: a sidecar file that exists but fails to parse must never be
 * reported as JAVA. For a diagnostic whose entire point is being
 * trustworthy, a wrong answer is worse than an admitted "don't know".
 */
public enum ChunkProvenance {
    OXIDE,
    JAVA,
    UNKNOWN
}
