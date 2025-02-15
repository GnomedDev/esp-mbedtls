//! Example for a sync server.
//! Contains a basic server implementation to test mbedtls in server mode.
//!
//! This example uses self-signed certificate. Your browser may display an error.
//! You have to enable the exception to then proceed, of if using curl, use the flag `-k`.
#![no_std]
#![no_main]

#[doc(hidden)]
pub use esp_hal as hal;

use embedded_io::*;
use esp_backtrace as _;
use esp_mbedtls::{set_debug, Mode, TlsError, TlsVersion, X509};
use esp_mbedtls::{Certificates, Session};
use esp_println::{logger::init_logger, print, println};
use esp_wifi::{
    current_millis, initialize,
    wifi::{utils::create_network_interface, ClientConfiguration, Configuration, WifiStaDevice},
    wifi_interface::WifiStack,
    EspWifiInitFor,
};
use hal::{
    clock::ClockControl, peripherals::Peripherals, prelude::*, rng::Rng, system::SystemControl,
};
use smoltcp::iface::SocketStorage;

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");

#[entry]
fn main() -> ! {
    init_logger(log::LevelFilter::Info);

    let mut peripherals = Peripherals::take();
    let system = SystemControl::new(peripherals.SYSTEM);
    let clocks = ClockControl::max(system.clock_control).freeze();

    #[cfg(target_arch = "xtensa")]
    let timer = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG1, &clocks, None).timer0;
    #[cfg(target_arch = "riscv32")]
    let timer = esp_hal::timer::systimer::SystemTimer::new(peripherals.SYSTIMER).alarm0;
    let init = initialize(
        EspWifiInitFor::Wifi,
        timer,
        Rng::new(peripherals.RNG),
        peripherals.RADIO_CLK,
        &clocks,
    )
    .unwrap();

    let wifi = peripherals.WIFI;
    let mut socket_set_entries: [SocketStorage; 3] = Default::default();
    let (iface, device, mut controller, sockets) =
        create_network_interface(&init, wifi, WifiStaDevice, &mut socket_set_entries).unwrap();
    let wifi_stack = WifiStack::new(iface, device, sockets, current_millis);

    println!("Call wifi_connect");
    let client_config = Configuration::Client(ClientConfiguration {
        ssid: SSID.try_into().unwrap(),
        password: PASSWORD.try_into().unwrap(),
        ..Default::default()
    });
    controller.set_configuration(&client_config).unwrap();
    controller.start().unwrap();
    controller.connect().unwrap();

    println!("Wait to get connected");
    loop {
        let res = controller.is_connected();
        match res {
            Ok(connected) => {
                if connected {
                    break;
                }
            }
            Err(err) => {
                println!("{:?}", err);
                loop {}
            }
        }
    }

    // wait for getting an ip address
    println!("Wait to get an ip address");
    loop {
        wifi_stack.work();

        if wifi_stack.is_iface_up() {
            println!("Got ip {:?}", wifi_stack.get_ip_info());
            break;
        }
    }

    println!("We are connected!");

    println!(
        "Point your browser to https://{:?}/",
        wifi_stack.get_ip_info().unwrap().ip
    );
    let mut rx_buffer = [0u8; 1536];
    let mut tx_buffer = [0u8; 1536];
    let mut socket = wifi_stack.get_socket(&mut rx_buffer, &mut tx_buffer);

    socket.listen(443).unwrap();
    set_debug(0);

    loop {
        socket.work();

        if !socket.is_open() {
            socket.listen(443).unwrap();
        }

        if socket.is_connected() {
            println!("New connection");

            let mut time_out = false;
            let wait_end = current_millis() + 20 * 1000;
            let mut buffer = [0u8; 1024];
            let mut pos = 0;

            let tls = Session::new(
                &mut socket,
                "",
                Mode::Server,
                TlsVersion::Tls1_2,
                Certificates {
                    // Use self-signed certificates
                    certificate: X509::pem(
                        concat!(include_str!("./certs/certificate.pem"), "\0").as_bytes(),
                    )
                    .ok(),
                    private_key: X509::pem(
                        concat!(include_str!("./certs/private_key.pem"), "\0").as_bytes(),
                    )
                    .ok(),
                    ..Default::default()
                },
            )
            .unwrap()
            .with_hardware_rsa(&mut peripherals.RSA);

            match tls.connect() {
                Ok(mut connected_session) => {
                    loop {
                        if let Ok(len) = connected_session.read(&mut buffer[pos..]) {
                            let to_print =
                                unsafe { core::str::from_utf8_unchecked(&buffer[..(pos + len)]) };

                            if to_print.contains("\r\n\r\n") {
                                print!("{}", to_print);
                                println!();
                                break;
                            }

                            pos += len;
                        } else {
                            break;
                        }

                        if current_millis() > wait_end {
                            println!("Timed out");
                            time_out = true;
                            break;
                        }
                    }

                    if !time_out {
                        connected_session
                            .write_all(
                                b"HTTP/1.0 200 OK\r\n\r\n\
                                    <html>\
                                    <body>\
                                    <h1>Hello Rust! Hello esp-mbedtls!</h1>\
                                    </body>\
                                    </html>\r\n\
                                    ",
                            )
                            .unwrap();
                    }

                    drop(connected_session);
                }
                Err(TlsError::MbedTlsError(-30592)) => {
                    println!("Fatal message: Please enable the exception for a self-signed certificate in your browser");
                }
                Err(error) => {
                    panic!("{:?}", error);
                }
            }

            socket.close();

            println!("Done\n");
            println!();
        }
    }
}
