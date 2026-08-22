package dev.oxide.plugin;

import dev.oxide.plugin.provenance.ChunkProvenance;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;

/**
 * Renders a ChunkProvenance as the action-bar Component. Adventure
 * Component/NamedTextColor — Paper's native rich-text API — never legacy
 * ChatColor or raw Strings.
 */
public final class ActionBarPresenter {

    private ActionBarPresenter() {
    }

    public static Component render(ChunkProvenance provenance) {
        return switch (provenance) {
            case OXIDE -> Component.text("chunkgen: oxide", NamedTextColor.AQUA);
            case JAVA -> Component.text("chunkgen: java", NamedTextColor.GOLD);
            case UNKNOWN -> Component.text("chunkgen: unknown", NamedTextColor.RED);
        };
    }
}
