#[deny(warnings)]
//use log::LevelFilter;
use speedy2d::color::Color;
use speedy2d::font::{Font, FormattedTextBlock, TextLayout, TextOptions};
use speedy2d::window::{WindowHandler, WindowHelper};
use speedy2d::{Graphics2D, Window};
use speedy2d::window::VirtualKeyCode;
use speedy2d::window::KeyScancode;
use portable_pty::{CommandBuilder, PtySize, native_pty_system, PtySystem};
use anyhow::Error;
use std::sync::mpsc::{self, Receiver};
use std::io::Read;
use std::io::Write;
use std::thread;
use vte::{Parser, Perform};

struct TerminalState {
    content: String,
}

impl Perform for TerminalState {
    fn print(&mut self, c: char) {
        // VTE ya decodificó el caracter por ti
        self.content.push(c);
    }

    fn execute(&mut self, byte: u8) {
        // Manejamos saltos de línea y retornos de carro
        match byte {
            b'\n' => self.content.push('\n'),
            b'\r' => {
                // En un terminal real, esto movería el cursor al inicio
                // Por ahora lo ignoramos o manejamos como \n si el buffer es simple
            },
            b'\t' => self.content.push_str("    "),
            _ => {}
        }
    }
    
    fn csi_dispatch(
        &mut self, 
        params: &vte::Params, 
        _intermediates: &[u8], 
        _ignore: bool, 
        action: char
    ) {
        match action {
            // 'J' es para borrar pantalla (Erase in Display)
            'J' => {
                for param in params {
                    // En versiones recientes, cada 'param' se puede convertir en un iterador
                    // o directamente comparar si el primer valor es 2.
                    if param.get(0) == Some(&2) {
                        self.content.clear();
                    }
                }
            },
            // 'm' es para colores y estilos (SGR)
            'm' => {
                // Aquí VTE nos manda los códigos de color
                for param in params {
                    match param.get(0) {
                        Some(&0) => { /* Resetear color */ },
                        Some(&31) => { /* Cambiar a Rojo */ },
                        _ => {}
                    }
                }
            },
            _ => {}
        }
    }
}
fn main() {
    // 1. Configurar el sistema PTY
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }).unwrap();

    // 2. Lanzar el shell
    let cmd = CommandBuilder::new("/bin/sh");
    let _child = pair.slave.spawn_command(cmd).unwrap();

    // 3. Crear un canal para comunicar el hilo de lectura con el de la ventana
    let (tx, rx) = mpsc::channel();
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut master_writer = pair.master.take_writer().unwrap();

    thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 { break; }
            // Enviamos los bytes crudos al hilo principal
            let s = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = tx.send(s);
        }
    });

    let window = Window::new_centered("Pitty Terminal - Raw PTY", (800, 600)).unwrap();
    let font = Font::new(include_bytes!("/home/almafuerte/.local/share/fonts/Mononoki/MononokiNerdFontMono-Regular.ttf")).unwrap();

    // Pasamos el receptor 'rx' al handler
    window.run_loop(MyWindowHandler { 
        font,
        rx,
        full_text: String::new(),
        display_text: None,
        master_writer,
        parser: Parser::new(),
        term_state: TerminalState { content: String::new() },
    })
}

struct MyWindowHandler {
    font: Font,
    rx: Receiver<String>,
    full_text: String,
    display_text: Option<FormattedTextBlock>,
    master_writer: Box<dyn Write + Send>,
    parser: Parser,
    term_state: TerminalState,
}

impl WindowHandler for MyWindowHandler {
    fn on_draw(&mut self, helper: &mut WindowHelper, graphics: &mut Graphics2D) {
        // 4. Revisar si hay nuevos datos en la PTY
        let mut updated = false;
        while let Ok(new_data) = self.rx.try_recv() {
            //self.full_text.push_str(&new_data);
            for byte in new_data.as_bytes() {
                self.parser.advance(&mut self.term_state, &[*byte]);
            }
            //self.parser.advance(&mut self.term_state, *byte);
            updated = true;
        }

        // Si hay datos nuevos, re-generamos el layout del texto
        if updated || self.display_text.is_none() {
            // Limitamos a las últimas 20 líneas para no colapsar sin scroll
            let lines: Vec<&str> = self.term_state.content.lines().rev().take(25).collect();
            //let lines: Vec<&str> = self.full_text.lines().rev().take(30).collect();
            let last_lines: String = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
            
            self.display_text = Some(self.font.layout_text(
                &last_lines, 
                16.0, 
                TextOptions::new()
            ));
        }

        graphics.clear_screen(Color::from_rgb(0.1, 0.1, 0.1));

        if let Some(text) = &self.display_text {
            graphics.draw_text((20.0, 20.0), Color::WHITE, text);
        }

        helper.request_redraw();
    }
    fn on_keyboard_char(&mut self, _helper: &mut WindowHelper, unicode_char: char) {
        // Cuando presionas una tecla normal (letras, números, símbolos)
        let _ = self.master_writer.write_all(unicode_char.to_string().as_bytes());
        let _ = self.master_writer.flush();
    }

    fn on_key_down(
        &mut self,
        _helper: &mut WindowHelper,
        virtual_key_code: Option<VirtualKeyCode>,
        _scancode: KeyScancode,
    ) {
        // Para teclas especiales que no son "caracteres" (Enter, Backspace, Flechas)
        if let Some(key) = virtual_key_code {
            let msg: &[u8] = match key {
                VirtualKeyCode::Return => b"\r",
                VirtualKeyCode::Backspace => b"\x7f", // Backspace en sistemas Unix
                VirtualKeyCode::Tab => b"\t",
                _ => b"",
            };
            
            if !msg.is_empty() {
                let _ = self.master_writer.write_all(msg);
                let _ = self.master_writer.flush();
            }
        }
    }
}
