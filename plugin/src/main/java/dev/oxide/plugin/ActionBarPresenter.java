package dev.oxide.plugin;

import dev.oxide.plugin.provenance.ChunkProvenance;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;

/**
 * Renders the unified Oxide & Pacside Action Bar HUD.
 * Uses Adventure Component / NamedTextColor.
 */
public final class ActionBarPresenter {

    private ActionBarPresenter() {
    }

    public static Component render(ChunkProvenance provenance) {
        return switch (provenance) {
            case OXIDE -> Component.text("● OXIDE (Rust)", NamedTextColor.AQUA);
            case JAVA -> Component.text("● JAVA (Vanilla)", NamedTextColor.GOLD);
            case UNKNOWN -> Component.text("● UNKNOWN", NamedTextColor.RED);
        };
    }

    public static Component renderUnified(int chunkX, int chunkZ, ChunkProvenance provenance, boolean isPrefetched, int lookahead, int cachedCount) {
        Component chunkPart = Component.text("[" + chunkX + ", " + chunkZ + "] ", NamedTextColor.WHITE);

        Component provPart = switch (provenance) {
            case OXIDE -> Component.text("● OXIDE (Rust)", NamedTextColor.AQUA);
            case JAVA -> Component.text("● JAVA (Vanilla)", NamedTextColor.GOLD);
            case UNKNOWN -> Component.text("● UNKNOWN", NamedTextColor.RED);
        };

        Component prefetchPart = isPrefetched
                ? Component.text(" | ● PRE-FETCHED (RAM Hit)", NamedTextColor.GREEN)
                : Component.text(" | ● STREAMED (Off-Heap)", NamedTextColor.DARK_AQUA);

        Component statsPart = Component.text(" | Lookahead: ", NamedTextColor.DARK_GRAY)
                .append(Component.text(lookahead + "c", NamedTextColor.YELLOW))
                .append(Component.text(" | Cached: ", NamedTextColor.DARK_GRAY))
                .append(Component.text(cachedCount, NamedTextColor.GRAY));

        return Component.text("", NamedTextColor.DARK_GRAY)
                .append(chunkPart)
                .append(provPart)
                .append(prefetchPart)
                .append(statsPart);
    }
}
