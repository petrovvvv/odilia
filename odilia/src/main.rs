mod cache;
mod events;
mod logging;
mod state;

use std::{process::exit, rc::Rc};

use eyre::WrapErr;
use futures::future::FutureExt;
use tokio::{
    signal::unix::{signal, SignalKind},
    sync::broadcast,
    sync::mpsc,
};

use crate::state::ScreenReaderState;
use atspi::accessible::Role;
use odilia_common::{
    events::{Direction, ScreenReaderEvent},
    modes::ScreenReaderMode,
};
use odilia_input::sr_event_receiver;
use ssip_client::Priority;

async fn sigterm_signal_watcher(shutdown_tx: broadcast::Sender<i32>) -> eyre::Result<()> {
    let mut c = signal(SignalKind::interrupt())?;
    tracing::debug!("Watching for Ctrl+C");
    c.recv().await;
    tracing::debug!("Asking all processes to stop.");
    let _ = shutdown_tx.send(0);
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> eyre::Result<()> {
    logging::init();
    // Make sure applications with dynamic accessibility supprt do expose their AT-SPI2 interfaces.
    if let  Err(e) = atspi::set_session_accessibility(true).await {
        tracing::debug!("Could not set AT-SPI2 IsEnabled property because: {}", e);
    }
    let _change_mode =
        ScreenReaderEvent::ChangeMode(ScreenReaderMode { name: "Browse".to_string() });
    let _sn = ScreenReaderEvent::StructuralNavigation(Direction::Forward, Role::Heading);
    let (shutdown_tx, _) = broadcast::channel(1);
    let (sr_event_tx, mut sr_event_rx) = mpsc::channel(8);
    // this channel must NEVER fill up; it will cause the thread receiving events to deadlock due to a zbus design choice.
    // If you need to make it bigger, then make it bigger, but do NOT let it ever fill up.
    let (atspi_event_tx, mut atspi_event_rx) = mpsc::channel(128);
    // this is the chanel which handles all SSIP commands. If SSIP is not allowed to operate on a separate task, then wdaiting for the receiving message can block other long-running operations like structural navigation.
    // Although in the future, this may possibly be remidied through a proper cache, I think it still makes sense to separate SSIP's IO operations to a separate task.
    // Like the channel above, it is very important that this is *never* full, since it can cause deadlocking if the other task sending the request is working with zbus.
    let (ssip_req_tx, mut ssip_req_rx) = mpsc::channel(32);
    // Initialize state
    let state = Rc::new(ScreenReaderState::new(&ssip_req_tx).await?);

    match state.say(Priority::Message, "Welcome to Odilia!".to_string()).await {
        true => tracing::debug!("Welcome message spoken."),
        false => {
            tracing::debug!("Welcome message failed. Odilia is not able to continue in this state. Existing now.");
            let _ = state.close_speech().await;
            exit(1);
        }
    };

    // Register events
    tokio::try_join!(
    state.register_event("Object:StateChanged:Focused"),
    state.register_event("Object:TextCaretMoved"),
    state.register_event("Document:LoadComplete"),
    )?;

		let mut shutdown_rx_ssip_recv = shutdown_tx.subscribe();
		/*let ssip_event_receiver = 
				handle_ssip_commands((*/
    let mut shutdown_rx_atspi_recv = shutdown_tx.subscribe();
    let atspi_event_receiver =
        events::receive(Rc::clone(&state), atspi_event_tx, &mut shutdown_rx_atspi_recv)
            .map(|_| Ok::<_, eyre::Report>(()));
    let mut shutdown_rx_atspi_proc_recv = shutdown_tx.subscribe();
    let atspi_event_processor =
        events::process(Rc::clone(&state), &mut atspi_event_rx, &mut shutdown_rx_atspi_proc_recv)
            .map(|_| Ok::<_, eyre::Report>(()));
    let mut shutdown_rx_odilia_recv = shutdown_tx.subscribe();
    let odilia_event_receiver = sr_event_receiver(sr_event_tx, &mut shutdown_rx_odilia_recv)
        .map(|r| r.wrap_err("Could not process Odilia events"));
    let mut shutdown_rx_odilia_proc_recv = shutdown_tx.subscribe();
    let odilia_event_processor =
        events::sr_event(Rc::clone(&state), &mut sr_event_rx, &mut shutdown_rx_odilia_proc_recv)
            .map(|r| r.wrap_err("Could not process Odilia event"));
    let signal_receiver = sigterm_signal_watcher(shutdown_tx)
        .map(|r| r.wrap_err("Could not process signal shutdown."));
    tokio::try_join!(
        signal_receiver,
        atspi_event_receiver,
        atspi_event_processor,
        odilia_event_receiver,
        odilia_event_processor
    )?;
    tracing::debug!("All listeners have stopped. Running cleanup code.");
    if  state.close_speech().await {
        tracing::debug!("Speech-dispatcher has successfully been stopped.");
    } else {
        tracing::debug!("Speech-dispatched has not been stopped; you may see problems when attempting to use it again.");
    }
    tracing::debug!("Goodbye, Odilia!");
    Ok(())
}
