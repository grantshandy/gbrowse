#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use argh::FromArgs;
use eframe::{
    egui::{self, containers::Frame, output::OpenUrl, style::Margin, RichText, ScrollArea},
    epaint::Color32,
    NativeOptions, Renderer,
};
use gmi::{
    gemtext::{self, GemtextNode},
    protocol::StatusCode,
    request,
    url::Url,
};

use std::{
    ffi::OsStr,
    path::PathBuf,
    str,
    sync::{
        mpsc::{self, Receiver, Sender, TryRecvError},
        Arc,
    },
    thread,
};

const DEFAULT_STARTING_PAGE: &'static str = "gemini://gemini.circumlunar.space";

fn main() {
    let mut options = NativeOptions::default();

    options.renderer = Renderer::Wgpu;

    eframe::run_native("gbrowse", options, Box::new(|_cc| Box::new(Gbrowse::new())));
}

#[derive(FromArgs)]
/// A simple gemini browser.
struct GbrowseArgs {
    /// what page to start on
    #[argh(option, short = 'p')]
    page: Option<String>,
}

struct Gbrowse {
    tx: Sender<Result<Vec<GemtextNode>, String>>,
    rx: Receiver<Result<Vec<GemtextNode>, String>>,
    sites: Vec<String>,
    content: Option<Vec<GemtextNode>>,
    error: Option<String>,
    loading: bool,
    url: String,
}

impl Gbrowse {
    pub fn new() -> Self {
        let args: GbrowseArgs = argh::from_env();

        let (tx, rx) = mpsc::channel();

        Self {
            tx,
            rx,
            sites: vec![],
            content: None,
            error: None,
            loading: false,
            url: args.page.unwrap_or(DEFAULT_STARTING_PAGE.to_string()),
        }
    }

    pub fn change_site(&mut self, url: &str, moving_back: bool) {
        self.error = None;
        self.content = None;

        println!("going to {url}");

        self.url = url.to_string();

        let url_structured = match Url::try_from(url) {
            Ok(url_structured) => url_structured,
            Err(err) => {
                self.error = Some(format!("Incorrectly formatted url: {err}"));

                return;
            }
        };

        let url = Arc::new(url_structured);
        let tx = self.tx.clone();

        if !moving_back {
            self.sites.push(self.url.clone());
        }

        self.loading = true;

        thread::spawn(move || {
            tx.send(make_request(&url)).unwrap();
        });
    }

    pub fn get_content(&mut self) -> Option<Vec<GemtextNode>> {
        match self.rx.try_recv() {
            Ok(content) => match content {
                Ok(content) => {
                    self.loading = false;
                    Some(content)
                }
                Err(err) => {
                    self.error = Some(err);
                    self.loading = false;

                    None
                }
            },
            Err(err) => {
                if err != TryRecvError::Empty {
                    self.error = Some(format!("Error Recieving From Other Thread: {err}"));
                    self.loading = false;
                }

                None
            }
        }
    }
}

impl eframe::App for Gbrowse {
    fn update(&mut self, ctx: &egui::Context, _fame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            // get content back from other thread
            if let Some(content) = self.get_content() {
                self.content = Some(content);
            }

            // search bar
            ScrollArea::horizontal()
                .id_source("horizontal scroll")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if self.sites.len() > 1 {
                            if ui.button("⏪").clicked() {
                                self.sites.pop();
                                if let Some(before) = self.sites.clone().last() {
                                    self.change_site(before, true);
                                }
                            }
                        }

                        ui.text_edit_singleline(&mut self.url);

                        if ui.button("🚀").clicked() {
                            self.change_site(&self.url.clone(), false);
                        }

                        if self.loading {
                            ui.label("loading...");
                        }
                    });
                });

            ui.separator();

            // display error
            if let Some(err) = &self.error {
                ui.label(RichText::new(err).color(Color32::RED).strong());
            }

            // display text
            if let Some(content) = &self.content.clone() {
                ScrollArea::vertical()
                    .id_source("vertical scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for block in content {
                            match block {
                                GemtextNode::Text(text) => {
                                    ui.label(text);
                                }
                                GemtextNode::Link(url, label) => {
                                    let link = ui
                                        .link(label.as_ref().unwrap_or(&url))
                                        .on_hover_text_at_pointer(url);

                                    if link.clicked() {
                                        // if full url
                                        if let Ok(parsed_url) = url::Url::parse(url) {
                                            if parsed_url.scheme() == "http"
                                                || parsed_url.scheme() == "https"
                                            {
                                                ui.ctx().output().open_url =
                                                    Some(OpenUrl::new_tab(url));
                                            // if gemini link
                                            } else if parsed_url.scheme() == "gemini" {
                                                self.change_site(url.as_str(), false);
                                            }
                                        } else {
                                            // if relative url
                                            let mut new_url =
                                                url::Url::parse(&self.url.clone()).unwrap();
                                            let mut new_path =
                                                PathBuf::from(new_url.path());

                                            let addition = PathBuf::from(url.clone());

                                            if addition.is_absolute() {
                                                new_path = addition;
                                            } else {
                                                if addition.extension() == Some(OsStr::new("gmi")) {
                                                    new_path.pop();
                                                }

                                                new_path.push(addition);
                                            }

                                            new_url.set_path(
                                                new_path.to_str().unwrap_or_default(),
                                            );
                                            self.change_site(new_url.as_str(), false);
                                        }
                                    }
                                }
                                GemtextNode::Heading(text) => {
                                    ui.label(RichText::new(text).size(30.0));
                                }
                                GemtextNode::SubHeading(text) => {
                                    ui.label(RichText::new(text).size(25.0));
                                }
                                GemtextNode::SubSubHeading(text) => {
                                    ui.label(RichText::new(text).size(20.0));
                                }
                                GemtextNode::ListItem(text) => {
                                    ui.label(format!("  • {text}"));
                                }
                                GemtextNode::Blockquote(text) => {
                                    let frame = Frame {
                                        outer_margin: Margin {
                                            left: 15.0,
                                            ..Margin::default()
                                        },
                                        ..Frame::default()
                                    };

                                    frame.show(ui, |ui| {
                                        ui.label(text);
                                    });
                                }
                                GemtextNode::Preformatted(text, _) => {
                                    ui.code(text);
                                }
                                GemtextNode::EmptyLine => {
                                    ui.add_space(10.0);
                                }
                            };
                        }
                    });
            }
        });

        if self.loading {
            ctx.request_repaint();
        }
    }
}

fn make_request(url: &Url) -> Result<Vec<GemtextNode>, String> {
    let mut url = url.clone();

    let data: Vec<u8> = loop {
        let response = match request::make_request(&url) {
            Ok(response) => response,
            Err(err) => return Err(format!("Request Error: {err}")),
        };

        match response.status {
            StatusCode::Redirect(_) => url = Url::try_from(response.meta.as_str()).unwrap(),
            StatusCode::Success(_) => break response.data,
            s => return Err(format!("Error: unknown status code: {:?}", s)),
        }
    };

    let text = match str::from_utf8(&data) {
        Ok(text) => text,
        Err(err) => return Err(format!("Text Formatting Error: {err}")),
    };

    let gemtext = gemtext::parse_gemtext(text);

    Ok(gemtext)
}
