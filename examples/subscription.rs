extern crate clightning_plugin_api;

use clightning_plugin_api::{ConnectEvent, DisconnectEvent, NoOptions, Plugin};


fn main() {
    let mut plugin = Plugin::<NoOptions, ()>::new()
        .subscribe(|_, event: ConnectEvent| {
            eprintln!("Received connect event from: {}@{:?}", event.id, event.address);
        })
        .subscribe(|_, event: DisconnectEvent| {
            eprintln!("Received disconnect event from: {}", event.id);
        });

    plugin.run();
}