// SPDX-License-Identifier: GPL-3.0-only

//! A second launch hands its files to the window already running.
//!
//! The real binary, against a real session bus — a private `dbus-daemon`, so
//! the user's session is never touched. A stand-in for the running window
//! owns the application's name and records what it is sent; the second
//! launch is `magnetar-pencil` itself, which must deliver its command line
//! and exit rather than open a window of its own.
//!
//! What the running window then does with the message is tested where the
//! window's state is: `app::tests`, through `dbus_activation`.

// The interface's platform data is of no interest here, so its arguments are
// underscored; the code `zbus::interface` generates still passes them along,
// which is the "use" this lint sees.
#![allow(clippy::used_underscore_binding)]

use std::collections::HashMap;
use std::io::BufRead as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use tokio::sync::mpsc;
use zbus::zvariant::OwnedValue;

/// A `dbus-daemon` of our own, torn down on drop.
struct PrivateBus {
    child: std::process::Child,
    address: String,
    _dir: tempfile::TempDir,
}

impl PrivateBus {
    /// Starts a bus. Panics when `dbus-daemon` is not installed: a test that
    /// needs a bus must fail loudly rather than pass without one.
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory for the bus");
        let config = dir.path().join("bus.conf");
        std::fs::write(
            &config,
            // `/tmp` rather than the temporary directory: a socket path has
            // a 108-byte limit, and `TMPDIR` can be longer than that.
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        )
        .expect("writing the bus configuration");

        let mut child = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .arg("--print-address=1")
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon must be installed to run the handover tests");
        let stdout = child.stdout.take().expect("the bus's stdout");
        let mut address = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut address)
            .expect("the bus prints its address");

        Self {
            child,
            address: address.trim().to_owned(),
            _dir: dir,
        }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// What the running window was sent.
#[derive(Debug, PartialEq, Eq)]
enum Call {
    Activate,
    Open(Vec<String>),
    Action(String, Vec<String>),
}

/// The running window's side of libcosmic's activation interface, recording
/// instead of acting.
struct Running(mpsc::UnboundedSender<Call>);

#[zbus::interface(name = "org.freedesktop.DbusActivation")]
impl Running {
    fn activate(&self, _platform_data: HashMap<String, OwnedValue>) {
        let _ = self.0.send(Call::Activate);
    }

    fn open(&self, uris: Vec<String>, _platform_data: HashMap<String, OwnedValue>) {
        let _ = self.0.send(Call::Open(uris));
    }

    fn activate_action(
        &self,
        action_name: String,
        parameter: Vec<String>,
        _platform_data: HashMap<String, OwnedValue>,
    ) {
        let _ = self.0.send(Call::Action(action_name, parameter));
    }
}

/// A window "running" on `bus`: the name is owned and the interface served
/// for as long as the connection lives.
async fn running(bus: &PrivateBus) -> (zbus::Connection, mpsc::UnboundedReceiver<Call>) {
    let (calls, received) = mpsc::unbounded_channel();
    let building = zbus::connection::Builder::address(bus.address.as_str())
        .expect("a valid bus address")
        .serve_at("/com/magnetaros/Pencil", Running(calls))
        .expect("serving the activation interface")
        .name(pencil::APP_ID)
        .expect("a valid bus name")
        .build();
    // Bounded: a bus that never answers must fail the test, not hang it.
    let connection = tokio::time::timeout(Duration::from_secs(10), building)
        .await
        .expect("the private bus answers")
        .expect("connecting to the private bus");
    (connection, received)
}

/// Runs `magnetar-pencil` with `arguments` in `directory`, on `bus`.
///
/// No display: if the handover did not happen the process fails at once
/// instead of opening a window on the desktop of whoever runs the tests. No
/// home but the scratch one either.
async fn launch(bus: &PrivateBus, directory: &Path, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_magnetar-pencil"));
    command
        .args(arguments)
        .current_dir(directory)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("HOME", directory)
        .env("XDG_STATE_HOME", directory.join("state"))
        .env("XDG_CONFIG_HOME", directory.join("config"))
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("COSMIC_SINGLE_INSTANCE")
        .env_remove("XDG_ACTIVATION_TOKEN")
        .env_remove("DESKTOP_STARTUP_ID");
    tokio::task::spawn_blocking(move || command.output())
        .await
        .expect("the launch ran")
        .expect("magnetar-pencil starts")
}

async fn next(received: &mut mpsc::UnboundedReceiver<Call>) -> Call {
    tokio::time::timeout(Duration::from_secs(10), received.recv())
        .await
        .expect("the running window was sent nothing")
        .expect("the running window is still there")
}

#[tokio::test(flavor = "multi_thread")]
async fn files_named_on_a_second_launch_reach_the_running_window() {
    let bus = PrivateBus::start();
    let (_window, mut received) = running(&bus).await;
    let scratch = tempfile::tempdir().unwrap();
    let here = scratch.path().canonicalize().unwrap();

    let output = launch(&bus, &here, &["notes.md", "/srv/docs/read me.html"]).await;
    assert!(
        output.status.success(),
        "the second launch did not hand over and exit: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let Call::Action(action, arguments) = next(&mut received).await else {
        panic!("the files were not sent as the open action");
    };
    assert_eq!(action, pencil::launch::OPEN);
    // What the running window makes of them: the same files, whatever
    // directory it was itself started in.
    assert_eq!(
        pencil::launch::paths(&arguments),
        [
            here.join("notes.md"),
            PathBuf::from("/srv/docs/read me.html")
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_launch_with_no_files_only_raises_the_running_window() {
    let bus = PrivateBus::start();
    let (_window, mut received) = running(&bus).await;
    let scratch = tempfile::tempdir().unwrap();

    let output = launch(&bus, scratch.path(), &[]).await;
    assert!(output.status.success());
    assert_eq!(next(&mut received).await, Call::Activate);
}

/// New window means a window: it is not handed to the one already running.
/// With no display to open it on, the launch fails — having sent nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_window_is_not_handed_to_the_running_one() {
    let bus = PrivateBus::start();
    let (_window, mut received) = running(&bus).await;
    let scratch = tempfile::tempdir().unwrap();

    let output = launch(&bus, scratch.path(), &[pencil::launch::NEW_WINDOW]).await;
    assert!(!output.status.success(), "a window opened with no display");
    assert!(
        received.try_recv().is_err(),
        "the running window was sent the new window's launch"
    );
}
