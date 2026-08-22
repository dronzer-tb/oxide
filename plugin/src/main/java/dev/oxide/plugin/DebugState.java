package dev.oxide.plugin;

import java.util.Set;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Per-player on/off state for the debug action-bar overlay. Default off.
 *
 * Backed by ConcurrentHashMap.newKeySet(): a toggle happens from whichever
 * region thread is handling that player's /oxide debug command, and a read
 * happens from whichever region thread is handling that player's movement.
 * These are frequently different Folia region threads (and can even be the
 * global/command-dispatch thread for the write side — see OxideCommand),
 * so this needs to be safe with no external locking.
 */
public final class DebugState {

    private final Set<UUID> enabled = ConcurrentHashMap.newKeySet();

    public void setEnabled(UUID player, boolean value) {
        if (value) {
            enabled.add(player);
        } else {
            enabled.remove(player);
        }
    }

    public boolean isEnabled(UUID player) {
        return enabled.contains(player);
    }

    public void clear(UUID player) {
        enabled.remove(player);
    }
}
