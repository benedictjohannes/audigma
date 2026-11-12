//! PipeWire registry probe: lists the `Audio/Sink` nodes a `target.object` hint can name.
//!
//! The probe is read-only. It connects to the default PipeWire daemon, receives the registry
//! snapshot, waits for one core round trip and disconnects. Nothing is created in the graph.

use std::cell::RefCell;
use std::rc::Rc;

use pipewire as pw;
use serde::{Deserialize, Serialize};

use crate::pipewire::{AUDIGMA_NODE_PROP, AUDIGMA_SINK_PREFIX};

/// One `Audio/Sink` node found in the registry.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PipeWireSinkInfo {
    /// Registry object id (changes between sessions; not usable in configuration).
    pub id: u32,
    /// `node.name`, the value for `output.target_node`.
    pub node_name: String,
    /// `node.description`, or `node.nick` when no description is set.
    pub description: Option<String>,
    /// `device.api` of the backing device (e.g. `alsa`), when the node has one.
    pub device_api: Option<String>,
    /// ALSA card index (`alsa.card`); matches `AlsaCardReport::index` from the ALSA probe.
    pub alsa_card: Option<u32>,
    /// ALSA card name (`alsa.card_name`).
    pub alsa_card_name: Option<String>,
    /// ALSA path of the node (`api.alsa.path`, e.g. `hw:Generic,9`).
    pub alsa_path: Option<String>,
}

/// Result of a PipeWire registry probe.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PipeWireProbeReport {
    /// Sinks other than audigma's own virtual sinks, ordered by `node.name`.
    pub sinks: Vec<PipeWireSinkInfo>,
}

/// Builds a sink entry from the full node properties (node info, not the registry
/// announcement). `None` when the node is not an `Audio/Sink`, has no name, or is one of
/// audigma's own virtual sinks.
pub fn sink_from_props<'a>(
    id: u32,
    get: impl Fn(&str) -> Option<&'a str>,
) -> Option<PipeWireSinkInfo> {
    if get("media.class") != Some("Audio/Sink") {
        return None;
    }
    let node_name = get("node.name").filter(|n| !n.is_empty())?;
    if get(AUDIGMA_NODE_PROP).is_some() || node_name.starts_with(AUDIGMA_SINK_PREFIX) {
        return None;
    }
    let own = |k: &str| get(k).filter(|v| !v.is_empty()).map(str::to_string);
    Some(PipeWireSinkInfo {
        id,
        node_name: node_name.to_string(),
        description: own("node.description").or_else(|| own("node.nick")),
        device_api: own("device.api"),
        alsa_card: get("alsa.card").and_then(|v| v.parse().ok()),
        alsa_card_name: own("alsa.card_name"),
        alsa_path: own("api.alsa.path"),
    })
}

/// Lists the registry's `Audio/Sink` nodes, excluding audigma's own virtual sinks.
///
/// The registry announces only a few node properties (name, description, `media.class`,
/// `device.id`), so each candidate sink is bound once to read its full property set. That costs
/// one extra core round trip.
pub fn probe_pipewire_sinks() -> Result<PipeWireProbeReport, String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| format!("main loop: {e}"))?;
    let context =
        pw::context::ContextRc::new(&mainloop, None).map_err(|e| format!("context: {e}"))?;
    let core = context
        .connect_rc(None)
        .map_err(|e| format!("cannot connect to the PipeWire daemon: {e}"))?;
    let registry = core
        .get_registry_rc()
        .map_err(|e| format!("registry: {e}"))?;
    let registry_weak = registry.downgrade();

    let sinks = Rc::new(RefCell::new(Vec::new()));
    // Bound proxies and their listeners must outlive the round trips.
    let bound: Rc<RefCell<Vec<(pw::node::Node, pw::node::NodeListener)>>> =
        Rc::new(RefCell::new(Vec::new()));
    let stage = Rc::new(std::cell::Cell::new(0u8));

    let pending = Rc::new(std::cell::Cell::new(
        core.sync(0).map_err(|e| format!("sync: {e}"))?,
    ));
    let core_listener = core
        .add_listener_local()
        .done({
            let stage = stage.clone();
            let pending = pending.clone();
            let mainloop = mainloop.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending.get() {
                    stage.set(stage.get() + 1);
                    mainloop.quit();
                }
            }
        })
        .register();
    let registry_listener = registry
        .add_listener_local()
        .global({
            let sinks = sinks.clone();
            let bound = bound.clone();
            move |global| {
                if global.type_ != pw::types::ObjectType::Node {
                    return;
                }
                let Some(props) = global.props else { return };
                if props.get("media.class") != Some("Audio/Sink") {
                    return;
                }
                let Some(registry) = registry_weak.upgrade() else {
                    return;
                };
                let Ok(node) = registry.bind::<pw::node::Node, _>(global) else {
                    return;
                };
                let id = global.id;
                let sinks = sinks.clone();
                let listener = node
                    .add_listener_local()
                    .info(move |info| {
                        if let Some(props) = info.props()
                            && let Some(sink) = sink_from_props(id, |k| props.get(k))
                        {
                            let mut sinks = sinks.borrow_mut();
                            // `info` fires again on property changes; keep the latest.
                            sinks.retain(|s: &PipeWireSinkInfo| s.id != id);
                            sinks.push(sink);
                        }
                    })
                    .register();
                bound.borrow_mut().push((node, listener));
            }
        })
        .register();

    // Round trip 1: the registry snapshot (binds are queued while it arrives).
    while stage.get() < 1 {
        mainloop.run();
    }
    // Round trip 2: the info events of the bound nodes.
    pending.set(core.sync(0).map_err(|e| format!("sync: {e}"))?);
    while stage.get() < 2 {
        mainloop.run();
    }
    drop(registry_listener);
    drop(core_listener);
    drop(bound);

    let mut sinks = sinks.take();
    sinks.sort_by(|a, b| a.node_name.cmp(&b.node_name));
    Ok(PipeWireProbeReport { sinks })
}

/// Names of the nodes that the node `stream_node` sends audio to, according to the links
/// in the graph. `nodes` holds `(object id, node.name)` and `links` holds
/// `(output node id, input node id)`. Sorted and de-duplicated.
pub fn link_targets(
    nodes: &[(u32, String)],
    links: &[(u32, u32)],
    stream_node: &str,
) -> Vec<String> {
    let name = |id: u32| {
        nodes
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.as_str())
    };
    let mut targets: Vec<String> = links
        .iter()
        .filter(|(out, _)| name(*out) == Some(stream_node))
        .filter_map(|(_, inp)| name(*inp).map(str::to_string))
        .collect();
    targets.sort();
    targets.dedup();
    targets
}

/// Names of the nodes that the PipeWire node `stream_node` is currently linked to (as the
/// producer). Empty when the node does not exist or has no links yet.
///
/// Uses only registry-announced properties (`node.name`, `link.output.node`,
/// `link.input.node`), so no nodes are bound.
pub fn output_link_targets(stream_node: &str) -> Result<Vec<String>, String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| format!("main loop: {e}"))?;
    let context =
        pw::context::ContextRc::new(&mainloop, None).map_err(|e| format!("context: {e}"))?;
    let core = context
        .connect_rc(None)
        .map_err(|e| format!("cannot connect to the PipeWire daemon: {e}"))?;
    let registry = core
        .get_registry_rc()
        .map_err(|e| format!("registry: {e}"))?;

    let nodes = Rc::new(RefCell::new(Vec::<(u32, String)>::new()));
    let links = Rc::new(RefCell::new(Vec::<(u32, u32)>::new()));
    let done = Rc::new(std::cell::Cell::new(false));
    let pending = core.sync(0).map_err(|e| format!("sync: {e}"))?;
    let core_listener = core
        .add_listener_local()
        .done({
            let done = done.clone();
            let mainloop = mainloop.clone();
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    done.set(true);
                    mainloop.quit();
                }
            }
        })
        .register();
    let registry_listener = registry
        .add_listener_local()
        .global({
            let nodes = nodes.clone();
            let links = links.clone();
            move |global| {
                let Some(props) = global.props else { return };
                match global.type_ {
                    pw::types::ObjectType::Node => {
                        if let Some(name) = props.get("node.name") {
                            nodes.borrow_mut().push((global.id, name.to_string()));
                        }
                    }
                    pw::types::ObjectType::Link => {
                        let id = |k: &str| props.get(k).and_then(|v| v.parse::<u32>().ok());
                        if let (Some(out), Some(inp)) =
                            (id("link.output.node"), id("link.input.node"))
                        {
                            links.borrow_mut().push((out, inp));
                        }
                    }
                    _ => {}
                }
            }
        })
        .register();
    while !done.get() {
        mainloop.run();
    }
    drop(registry_listener);
    drop(core_listener);
    let (nodes, links) = (nodes.take(), links.take());
    Ok(link_targets(&nodes, &links, stream_node))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sink(props: &[(&str, &str)]) -> Option<PipeWireSinkInfo> {
        let map: HashMap<_, _> = props.iter().copied().collect();
        sink_from_props(7, |k| map.get(k).copied())
    }

    #[test]
    fn keeps_hardware_sink_with_description() {
        let s = sink(&[
            ("media.class", "Audio/Sink"),
            ("node.name", "alsa_output.usb-FiiO_K17"),
            ("node.description", "K17 Analog Stereo"),
            ("device.api", "alsa"),
            ("alsa.card", "3"),
            ("alsa.card_name", "K17"),
            ("api.alsa.path", "hw:K17"),
        ])
        .unwrap();
        assert_eq!(s.node_name, "alsa_output.usb-FiiO_K17");
        assert_eq!(s.description.as_deref(), Some("K17 Analog Stereo"));
        assert_eq!(s.alsa_card, Some(3));
        assert_eq!(s.alsa_card_name.as_deref(), Some("K17"));
        assert_eq!(s.alsa_path.as_deref(), Some("hw:K17"));
    }

    #[test]
    fn falls_back_to_nick_and_drops_empty_values() {
        let s = sink(&[
            ("media.class", "Audio/Sink"),
            ("node.name", "x"),
            ("node.description", ""),
            ("node.nick", "Nick"),
        ])
        .unwrap();
        assert_eq!(s.description.as_deref(), Some("Nick"));
        assert_eq!(s.device_api, None);
    }

    #[test]
    fn skips_non_sinks_and_nameless_nodes() {
        assert!(sink(&[("media.class", "Audio/Source"), ("node.name", "m")]).is_none());
        assert!(sink(&[("media.class", "Stream/Output/Audio"), ("node.name", "s")]).is_none());
        assert!(sink(&[("media.class", "Audio/Sink")]).is_none());
        assert!(sink(&[("media.class", "Audio/Sink"), ("node.name", "")]).is_none());
    }

    #[test]
    fn skips_audigma_sinks_by_tag_or_default_name() {
        assert!(
            sink(&[
                ("media.class", "Audio/Sink"),
                ("node.name", "custom_name"),
                (AUDIGMA_NODE_PROP, "true"),
            ])
            .is_none()
        );
        assert!(
            sink(&[
                ("media.class", "Audio/Sink"),
                ("node.name", "audigma_sink_default"),
            ])
            .is_none()
        );
    }

    #[test]
    fn link_targets_follow_producer_side_only() {
        let nodes = vec![
            (1, "audigma_pcm_out_default".to_string()),
            (2, "audigma_sink_default".to_string()),
            (3, "alsa_output.x".to_string()),
            (4, "mpd".to_string()),
        ];
        // out -> own sink twice (L and R); mpd -> own sink; own sink monitor -> hardware.
        let links = vec![(1, 2), (1, 2), (4, 2), (2, 3)];
        assert_eq!(
            link_targets(&nodes, &links, "audigma_pcm_out_default"),
            vec!["audigma_sink_default"]
        );
        assert_eq!(
            link_targets(&nodes, &links, "mpd"),
            vec!["audigma_sink_default"]
        );
        assert!(link_targets(&nodes, &links, "missing").is_empty());
    }

    #[test]
    fn link_targets_ignore_links_to_unnamed_nodes() {
        let nodes = vec![(1, "out".to_string())];
        assert!(link_targets(&nodes, &[(1, 9)], "out").is_empty());
    }
}
