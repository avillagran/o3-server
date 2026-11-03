#![cfg(target_os = "linux")]
#[path = "support/signals.rs"]
mod signals;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    time::Duration,
};

extern "C" fn child_signal(_: libc::c_int) {}

#[test]
fn fixture_client_read_is_not_interrupted_by_targeted_child_signal() {
    unsafe {
        let mut handler: libc::sigaction = std::mem::zeroed();
        handler.sa_sigaction = child_signal as *const () as usize;
        libc::sigemptyset(&mut handler.sa_mask);
        // Deliberately no SA_RESTART, as required to exercise an interrupted read.
        let mut old_handler: libc::sigaction = std::mem::zeroed();
        assert_eq!(
            libc::sigaction(libc::SIGCHLD, &handler, &mut old_handler),
            0
        );
        let mut old_mask: libc::sigset_t = std::mem::zeroed();
        assert_eq!(
            libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut old_mask),
            0
        );
        let mut unblocked = old_mask;
        libc::sigdelset(&mut unblocked, libc::SIGCHLD);
        for shielded in [false, true] {
            assert_eq!(
                libc::pthread_sigmask(libc::SIG_SETMASK, &unblocked, std::ptr::null_mut()),
                0
            );
            if shielded {
                signals::shield_client_thread_from_sigchld();
            }
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let (mut server, _) = listener.accept().unwrap();
            let target = libc::pthread_self();
            let sender = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                assert_eq!(libc::pthread_kill(target, libc::SIGCHLD), 0);
                std::thread::sleep(Duration::from_millis(30));
                server.write_all(b"x").unwrap();
            });
            let result = client.read(&mut [0u8; 1]);
            sender.join().unwrap();
            if shielded {
                assert_eq!(result.unwrap(), 1);
            } else {
                assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
            }
        }
        // Deliver the directed pending signal under our handler before restoring it.
        assert_eq!(
            libc::pthread_sigmask(libc::SIG_SETMASK, &unblocked, std::ptr::null_mut()),
            0
        );
        assert_eq!(
            libc::sigaction(libc::SIGCHLD, &old_handler, std::ptr::null_mut()),
            0
        );
        assert_eq!(
            libc::pthread_sigmask(libc::SIG_SETMASK, &old_mask, std::ptr::null_mut()),
            0
        );
    }
}
