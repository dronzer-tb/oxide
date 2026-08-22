package dev.oxide.plugin;

import dev.oxide.plugin.provenance.ProvenanceLookup;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import org.bukkit.command.Command;
import org.bukkit.command.CommandExecutor;
import org.bukkit.command.CommandSender;
import org.bukkit.command.TabCompleter;
import org.bukkit.entity.Player;
import org.bukkit.plugin.Plugin;

import java.util.List;

/**
 * {@code /oxide debug on|off} — toggles the per-player action-bar overlay.
 * {@code /oxide here}         — one-shot detail dump for the sender's current chunk.
 *
 * Threading: Folia does not guarantee a player-issued command is processed
 * on the region thread that currently owns that player — command dispatch
 * runs on whatever thread received the packet, which need not line up with
 * the sender entity's owning region. Every branch that reads the Player's
 * location or sends it a message is therefore wrapped in
 * {@code player.getScheduler().run(...)} (the per-entity scheduler) so it
 * always executes on the thread that currently owns that player.
 * {@code Bukkit.getScheduler()} is never used here — see ChunkTracker for
 * the fuller rationale, which applies identically to this class.
 */
public final class OxideCommand implements CommandExecutor, TabCompleter {

    private final Plugin plugin;
    private final DebugState debugState;
    private final ProvenanceLookup provenanceLookup;

    public OxideCommand(Plugin plugin, DebugState debugState, ProvenanceLookup provenanceLookup) {
        this.plugin = plugin;
        this.debugState = debugState;
        this.provenanceLookup = provenanceLookup;
    }

    @Override
    public boolean onCommand(CommandSender sender, Command command, String label, String[] args) {
        if (!(sender instanceof Player player)) {
            sender.sendMessage(Component.text("This command can only be used by a player.", NamedTextColor.RED));
            return true;
        }
        if (!player.hasPermission("oxide.debug")) {
            player.sendMessage(Component.text("You do not have permission to use this command.", NamedTextColor.RED));
            return true;
        }

        if (args.length == 2 && args[0].equalsIgnoreCase("debug")) {
            boolean on = args[1].equalsIgnoreCase("on");
            boolean off = args[1].equalsIgnoreCase("off");
            if (!on && !off) {
                player.sendMessage(Component.text("Usage: /oxide debug <on|off>", NamedTextColor.RED));
                return true;
            }
            // Toggling shared state doesn't touch the Player entity itself,
            // safe to do directly on whatever thread dispatched the command.
            debugState.setEnabled(player.getUniqueId(), on);
            player.getScheduler().run(plugin,
                    task -> player.sendMessage(Component.text(
                            "chunkgen debug overlay: " + (on ? "on" : "off"), NamedTextColor.GRAY)),
                    null);
            return true;
        }

        if (args.length == 1 && args[0].equalsIgnoreCase("here")) {
            player.getScheduler().run(plugin, task -> printHere(player), null);
            return true;
        }

        player.sendMessage(Component.text("Usage: /oxide debug <on|off> | /oxide here", NamedTextColor.RED));
        return true;
    }

    /** Runs on the player's owning entity scheduler — see class javadoc. */
    private void printHere(Player player) {
        int chunkX = player.getLocation().getBlockX() >> 4;
        int chunkZ = player.getLocation().getBlockZ() >> 4;
        ProvenanceLookup.Result result = provenanceLookup.lookup(
                player.getWorld().getName(), player.getWorld().getWorldFolder(), chunkX, chunkZ);

        player.sendMessage(Component.text("chunk: " + result.chunkX() + ", " + result.chunkZ(), NamedTextColor.GRAY));
        player.sendMessage(Component.text("region file: " + result.regionFile().getFileName(), NamedTextColor.GRAY));
        String sidecarLine = "sidecar present: " + result.sidecarPresent()
                + (result.sidecarParseError() ? " (parse error, see server log)" : "");
        player.sendMessage(Component.text(sidecarLine, NamedTextColor.GRAY));
        String bitLine = "bit: " + ((result.sidecarPresent() && !result.sidecarParseError())
                ? String.valueOf(result.bitSet()) : "n/a");
        player.sendMessage(Component.text(bitLine, NamedTextColor.GRAY));
        player.sendMessage(ActionBarPresenter.render(result.provenance()));
    }

    @Override
    public List<String> onTabComplete(CommandSender sender, Command command, String alias, String[] args) {
        if (args.length == 1) {
            return List.of("debug", "here");
        }
        if (args.length == 2 && args[0].equalsIgnoreCase("debug")) {
            return List.of("on", "off");
        }
        return List.of();
    }
}
