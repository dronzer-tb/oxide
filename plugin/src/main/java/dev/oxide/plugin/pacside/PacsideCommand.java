package dev.oxide.plugin.pacside;

import dev.oxide.plugin.DebugState;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.format.NamedTextColor;
import org.bukkit.command.Command;
import org.bukkit.command.CommandExecutor;
import org.bukkit.command.CommandSender;
import org.bukkit.command.TabCompleter;
import org.bukkit.entity.Player;
import org.bukkit.plugin.Plugin;

import java.util.ArrayList;
import java.util.List;

/**
 * Standalone command executor for /pacside.
 */
public final class PacsideCommand implements CommandExecutor, TabCompleter {

    private final Plugin plugin;
    private final PacsideManager pacsideManager;
    private final DebugState debugState;

    public PacsideCommand(Plugin plugin, PacsideManager pacsideManager, DebugState debugState) {
        this.plugin = plugin;
        this.pacsideManager = pacsideManager;
        this.debugState = debugState;
    }

    @Override
    public boolean onCommand(CommandSender sender, Command command, String label, String[] args) {
        if (!sender.hasPermission("oxide.pacside") && !sender.hasPermission("oxide.debug")) {
            sender.sendMessage(Component.text("You do not have permission to use this command.", NamedTextColor.RED));
            return true;
        }

        if (args.length == 0 || (args.length == 1 && args[0].equalsIgnoreCase("stats"))) {
            sender.sendMessage(Component.text("=== Pacside Off-Heap Chunk Streamer ===", NamedTextColor.GOLD));
            sender.sendMessage(Component.text("Prefetcher: ", NamedTextColor.GRAY)
                    .append(Component.text(pacsideManager.getPrefetcher().isEnabled() ? "ENABLED (24c Lookahead)" : "DISABLED",
                            pacsideManager.getPrefetcher().isEnabled() ? NamedTextColor.GREEN : NamedTextColor.RED)));
            sender.sendMessage(Component.text("Total Prefetched: ", NamedTextColor.GRAY)
                    .append(Component.text(pacsideManager.getPrefetcher().getPrefetchedCount(), NamedTextColor.AQUA)));
            sender.sendMessage(Component.text("Cached Chunks in RAM: ", NamedTextColor.GRAY)
                    .append(Component.text(pacsideManager.getPrefetcher().getCachedSetSize(), NamedTextColor.YELLOW)));
            sender.sendMessage(Component.text("Subcommands: ", NamedTextColor.DARK_GRAY)
                    .append(Component.text("/pacside hud [on|off] | /pacside fetch <radius> | /pacside clear | /pacside stats", NamedTextColor.GRAY)));
            return true;
        }

        String sub = args[0].toLowerCase();
        switch (sub) {
            case "hud" -> {
                if (!(sender instanceof Player player)) {
                    sender.sendMessage(Component.text("HUD toggle is only available for in-game players.", NamedTextColor.RED));
                    return true;
                }
                boolean targetState;
                if (args.length >= 2) {
                    targetState = args[1].equalsIgnoreCase("on") || args[1].equalsIgnoreCase("true");
                } else {
                    targetState = !pacsideManager.getPrefetcher().hasVisualHud(player.getUniqueId());
                }
                pacsideManager.getPrefetcher().toggleVisualHud(player.getUniqueId(), targetState);
                debugState.setEnabled(player.getUniqueId(), targetState);
                player.sendMessage(Component.text("Pacside & Oxide Unified HUD: ", NamedTextColor.GOLD)
                        .append(Component.text(targetState ? "ENABLED" : "DISABLED",
                                targetState ? NamedTextColor.GREEN : NamedTextColor.RED)));
                return true;
            }
            case "fetch" -> {
                if (!(sender instanceof Player player)) {
                    sender.sendMessage(Component.text("Radius fetch is only available for in-game players.", NamedTextColor.RED));
                    return true;
                }
                int radius = 32;
                if (args.length >= 2) {
                    try {
                        String raw = args[1].toLowerCase();
                        if (raw.equals("radius") && args.length >= 3) {
                            radius = Integer.parseInt(args[2]);
                        } else {
                            radius = Integer.parseInt(raw);
                        }
                    } catch (NumberFormatException e) {
                        sender.sendMessage(Component.text("Invalid radius number. Usage: /pacside fetch <radius>", NamedTextColor.RED));
                        return true;
                    }
                }
                pacsideManager.getPrefetcher().prefetchRadius(player, radius);
                return true;
            }
            case "clear" -> {
                pacsideManager.getPrefetcher().clearCache();
                PacsideNative.clear();
                sender.sendMessage(Component.text("Pacside off-heap chunk cache and prefetch index cleared.", NamedTextColor.GREEN));
                return true;
            }
            case "stress" -> {
                if (args.length >= 2 && args[1].equalsIgnoreCase("stop")) {
                    pacsideManager.getStressSimulator().stop();
                    sender.sendMessage(Component.text("Flight stress simulation stopped.", NamedTextColor.YELLOW));
                    return true;
                }
                if (args.length >= 2 && args[1].equalsIgnoreCase("fly")) {
                    int flyers = 16;
                    double speed = 35.0;
                    int duration = 60;
                    try {
                        if (args.length >= 3) flyers = Integer.parseInt(args[2]);
                        if (args.length >= 4) speed = Double.parseDouble(args[3]);
                        if (args.length >= 5) duration = Integer.parseInt(args[4]);
                    } catch (NumberFormatException e) {
                        sender.sendMessage(Component.text("Invalid numbers. Usage: /pacside stress fly [flyers] [speed] [duration]", NamedTextColor.RED));
                        return true;
                    }

                    org.bukkit.Location loc = (sender instanceof Player p) ? p.getLocation() :
                            sender.getServer().getWorlds().get(0).getSpawnLocation();
                    pacsideManager.getStressSimulator().start(loc.getWorld(), loc, flyers, speed, duration);
                    sender.sendMessage(Component.text("Flight stress simulation started.", NamedTextColor.GREEN));
                    return true;
                }
                sender.sendMessage(Component.text("Usage: /pacside stress fly [flyers] [speed_mps] [duration_sec] | /pacside stress stop", NamedTextColor.RED));
                return true;
            }
            default -> {
                sender.sendMessage(Component.text("Unknown subcommand. Usage: /pacside [hud|fetch|stats|clear|stress]", NamedTextColor.RED));
                return true;
            }
        }
    }

    @Override
    public List<String> onTabComplete(CommandSender sender, Command command, String label, String[] args) {
        List<String> list = new ArrayList<>();
        if (args.length == 1) {
            for (String sub : List.of("hud", "fetch", "stats", "clear", "stress")) {
                if (sub.startsWith(args[0].toLowerCase())) {
                    list.add(sub);
                }
            }
        } else if (args.length == 2 && args[0].equalsIgnoreCase("hud")) {
            for (String sub : List.of("on", "off")) {
                if (sub.startsWith(args[1].toLowerCase())) {
                    list.add(sub);
                }
            }
        } else if (args.length == 2 && args[0].equalsIgnoreCase("fetch")) {
            for (String sub : List.of("16", "32", "48", "64")) {
                if (sub.startsWith(args[1].toLowerCase())) {
                    list.add(sub);
                }
            }
        }
        return list;
    }
}
