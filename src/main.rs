extern crate crossterm;
extern crate crossterm_input;
extern crate dirs;
extern crate serde;
extern crate sxd_document;
extern crate sxd_xpath;
extern crate terminal_size;
extern crate ureq;

use crossterm_input::KeyEvent::{Char, Down, Left, Right, Up};
use crossterm_input::{input, InputEvent, RawScreen};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::min;
use std::env;
use std::fs;
use std::io;
use std::io::Error;
use std::io::ErrorKind::NotFound;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use sxd_document::dom::Element;
use sxd_document::parser;
use sxd_xpath::context::Context;
use sxd_xpath::{evaluate_xpath, Factory, Value};
use terminal_size::{terminal_size, Height, Width};
use webbrowser;

fn default_mpv_mode() -> bool {
    true
}

fn default_mpv_path() -> String {
    "/usr/bin/mpv".to_string()
}

#[derive(Serialize, Deserialize, Debug)]
struct AppConfig {
    video_path: String,
    cache_path: String,
    youtubedl_format: String,
    video_extension: String,
    players: Vec<Vec<String>>,
    channel_ids: Vec<String>,
    #[serde(default = "default_mpv_mode")]
    mpv_mode: bool,
    #[serde(default = "default_mpv_path")]
    mpv_path: String,
}

impl Default for AppConfig {
    fn default() -> AppConfig {
        AppConfig {
            video_path: "/tmp".to_string(),
            cache_path: "/tmp/yts.json".to_string(),
            youtubedl_format: "[height <=? 360][ext = mp4]".to_string(),
            video_extension: "mp4".to_string(),
            players: vec![
                vec![
                    "/usr/bin/omxplayer".to_string(),
                    "-o".to_string(),
                    "local".to_string(),
                ],
                vec![
                    "/Applications/VLC.app/Contents/MacOS/VLC".to_string(),
                    "--play-and-exit".to_string(),
                    "-f".to_string(),
                ],
                vec![
                    "/usr/bin/vlc".to_string(),
                    "--play-and-exit".to_string(),
                    "-f".to_string(),
                ],
                vec![
                    "/usr/bin/mpv".to_string(),
                    "-really-quiet".to_string(),
                    "-fs".to_string(),
                ],
                vec![
                    "/usr/bin/mplayer".to_string(),
                    "-really-quiet".to_string(),
                    "-fs".to_string(),
                ],
            ],
            channel_ids: vec![],
            mpv_mode: default_mpv_mode(),
            mpv_path: default_mpv_path(),
        }
    }
}

fn load_config() -> AppConfig {
    match dirs::home_dir() {
        Some(home) => {
            match home.to_str() {
                Some(h) => {
                    let path = format!("{}/.config/youtube-subscriptions/config.json", h);
                    match fs::read_to_string(path) {
                        Ok(s) => match serde_json::from_str::<AppConfig>(s.as_str()) {
                            Ok(mut _res) => {
                                _res.video_path = _res.video_path.replace("__HOME", &h);
                                match fs::create_dir_all(&_res.video_path) {
                                    Ok(_) => {
                                        _res.cache_path = _res.cache_path.replace("__HOME", &h);
                                        match Path::new(&_res.cache_path).parent() {
                                                Some(dirname) => match fs::create_dir_all(&dirname) {
                                                    Ok(_) => _res,
                                                    Err(e) => panic!("error while creating cache directory for {}: {:?}", &_res.cache_path, e)
                                                }
                                                None => panic!("failed to find dirname of {}", &_res.cache_path),
                                            }
                                    }
                                    Err(e) => panic!(
                                        "error while creating video path {}: {:?}",
                                        &_res.video_path, e
                                    ),
                                }
                            }
                            Err(e) => panic!("error parsing configuration: {:?}", e),
                        },
                        Err(_) => AppConfig {
                            ..Default::default()
                        },
                    }
                }
                None => AppConfig {
                    ..Default::default()
                },
            }
        }
        None => AppConfig {
            ..Default::default()
        },
    }
}

fn get_subscriptions_xml() -> Result<String, Error> {
    match dirs::home_dir() {
        Some(home) => match home.to_str() {
            Some(s) => {
                let path = format!("{}/.config/youtube-subscriptions/subscription_manager", s);
                if fs::metadata(&path).is_ok() {
                    return fs::read_to_string(path);
                } else {
                    let url = "https://www.youtube.com/subscription_manager?action_takeout=1";
                    let _res = webbrowser::open(&url);
                    panic!(
                        "configuration is missing
please download: {} (a browser window should be opened with it).
make it available as {} ",
                        url, path
                    )
                }
            }
            None => panic!("failed reading subscription_manager"),
        },
        None => panic!("failed reading subscription_manager"),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Video {
    channel: String,
    title: String,
    thumbnail: String,
    url: String,
    published: String,
    description: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct Videos {
    videos: Vec<Video>,
}

fn get_value(xpath: String, node: Element) -> String {
    let factory = Factory::new();
    let xpath = factory
        .build(xpath.as_str())
        .expect("Could not compile XPath");
    let xpath = xpath.expect("No XPath was compiled");
    let context = Context::new();
    return xpath
        .evaluate(&context, node)
        .unwrap_or(Value::String("".to_string()))
        .string()
        .to_string();
}

fn get_channel_videos(channel_url: String) -> Vec<Video> {
    let response = ureq::get(channel_url.replace("https:", "http:").as_str()).call();
    if response.ok() {
        let contents = response.into_string().unwrap();
        let package = parser::parse(contents.as_str()).expect("failed to parse XML");
        let document = package.as_document();
        let title = evaluate_xpath(
            &document,
            "string(/*[local-name() = 'feed']/*[local-name() = 'title']/text())",
        )
        .unwrap_or(Value::String("".to_string()))
        .string();
        match evaluate_xpath(
            &document,
            "/*[local-name() = 'feed']/*[local-name() = 'entry']",
        ) {
            Ok(val) => {
                if let Value::Nodeset(entries) = val {
                    entries.iter().flat_map( |entry|
                                     match entry.element() {
                                         Some(_element) =>
                                         {
                                             vec![Video {
                                                 channel: title.to_string(),
                                                 title: get_value("string(*[local-name() = 'title']/text())".to_string(), _element),
                                                 thumbnail: get_value("string(*[local-name() = 'group']/*[local-name() = 'thumbnail']/@url)".to_string(), _element),
                                                 url: get_value("string(*[local-name() = 'group']/*[local-name() = 'content']/@url)".to_string(), _element),
                                                 published: get_value("string(*[local-name() = 'published']/text())".to_string(), _element),
                                                 description: get_value("string(*[local-name() = 'group']/*[local-name() = 'description']/text())".to_string(), _element),
                                             }]
                                         },
                                         None => vec![]
                                         }
                                ).collect()
                } else {
                    vec![]
                }
            }
            Err(_) => {
                println!("aaaaa");
                vec![]
            }
        }
    } else {
        vec![]
    }
}

fn get_videos(xml: String, additional_channel_ids: &Vec<String>) -> Vec<Video> {
    let package = parser::parse(xml.as_str()).expect("failed to parse XML");
    let document = package.as_document();
    match evaluate_xpath(&document, "//outline/@xmlUrl") {
        Ok(value) => {
            if let Value::Nodeset(urls) = value {
                let mut urls_from_xml: Vec<String> = urls
                    .iter()
                    .flat_map(|url| match url.attribute() {
                        Some(attribute) => Some(attribute.value().to_string()),
                        None => None,
                    })
                    .collect::<Vec<String>>();
                let urls_from_additional = additional_channel_ids.iter().map(|id| {
                    "https://www.youtube.com/feeds/videos.xml?channel_id=".to_string() + id
                });
                urls_from_xml.extend(urls_from_additional);
                urls_from_xml
                    .par_iter()
                    .flat_map(|url| get_channel_videos(url.to_string()))
                    .collect::<Vec<Video>>()
            } else {
                vec![]
            }
        }
        Err(err) => {
            println!("{:?}", err);
            vec![]
        }
    }
}

fn to_show_videos(
    videos: &mut Vec<Video>,
    start: usize,
    end: usize,
    filter: &String,
) -> Vec<Video> {
    videos.sort_by(|a, b| b.published.cmp(&a.published));
    let filtered_videos = videos
        .iter()
        .filter(|video| {
            video.title.contains(filter.as_str()) || video.channel.contains(filter.as_str())
        })
        .cloned()
        .collect::<Vec<Video>>();
    let new_end = std::cmp::min(end, filtered_videos.len());
    let mut result = filtered_videos[start..new_end].to_vec();
    result.reverse();
    return result;
}

fn load(reload: bool, app_config: &AppConfig) -> Option<Videos> {
    match get_subscriptions_xml() {
        Ok(xml) => {
            let path = app_config.cache_path.as_str();
            if reload || !fs::metadata(path).is_ok() {
                let videos = Videos {
                    videos: get_videos(xml, &app_config.channel_ids),
                };
                let serialized = serde_json::to_string(&videos).unwrap();
                fs::write(path, serialized).expect("writing videos json failed");
            }
            match fs::read_to_string(path) {
                Ok(s) => Some(serde_json::from_str(s.as_str()).unwrap()),
                Err(_) => None,
            }
        }
        Err(_) => None,
    }
}

fn get_lines() -> usize {
    let size = terminal_size();
    if let Some((Width(_), Height(h))) = size {
        (h - 1) as usize
    } else {
        20
    }
}

fn get_cols() -> usize {
    let size = terminal_size();
    if let Some((Width(w), Height(_))) = size {
        w as usize
    } else {
        20
    }
}

fn hide_cursor() {
    print!("\x1b[?25l");
    io::stdout().flush().unwrap();
}

fn smcup() {
    print!("\x1b[?1049h");
    io::stdout().flush().unwrap();
}

fn rmcup() {
    print!("\x1b[?1049l");
    io::stdout().flush().unwrap();
}

fn clear() {
    print!("\x1b[2J");
    io::stdout().flush().unwrap();
}

fn show_cursor() {
    print!("\x1b[?25h");
    io::stdout().flush().unwrap();
}

fn move_cursor(i: usize) {
    print!("\x1b[{};0f", i + 1);
    io::stdout().flush().unwrap();
}

fn move_to_bottom() {
    print!("\x1b[{};0f", get_lines() + 1);
    io::stdout().flush().unwrap();
}

fn clear_to_end_of_line() {
    print!("\x1b[K");
    io::stdout().flush().unwrap();
}

fn debug(s: &String) {
    move_to_bottom();
    clear_to_end_of_line();
    move_to_bottom();
    print!("{}", s);
    io::stdout().flush().unwrap();
}

fn print_selector(i: usize) {
    move_cursor(i);
    print!("\x1b[1m|\x1b[0m\r");
    io::stdout().flush().unwrap();
}

fn clear_selector(i: usize) {
    move_cursor(i);
    print!(" ");
    io::stdout().flush().unwrap();
}

fn jump(i: usize, new_i: usize) -> usize {
    clear_selector(i);
    return new_i;
}

fn pause() {
    let input = input();
    let _screen = RawScreen::into_raw_mode();
    let _c = input.read_char();
}

struct YoutubeSubscribtions {
    n: usize,
    start: usize,
    filter: String,
    i: usize,
    toshow: Vec<Video>,
    videos: Videos,
    app_config: AppConfig,
}

fn print_videos(toshow: &Vec<Video>) {
    let max = toshow.iter().fold(0, |acc, x| {
        if x.channel.chars().count() > acc {
            x.channel.chars().count()
        } else {
            acc
        }
    });
    let cols = get_cols();
    for video in toshow.iter().rev() {
        let published = video.published.split("T").collect::<Vec<&str>>();
        let whitespaces = " ".repeat(max - video.channel.chars().count());
        let s = format!(
            "  \x1b[36m{}\x1b[0m \x1b[34m{}\x1b[0m{} {}",
            published[0][5..10].to_string(),
            video.channel,
            whitespaces,
            video.title
        );
        println!(
            "{}",
            s.chars()
                .take(min(s.chars().count(), cols - 4 + 9 + 9 + 2))
                .collect::<String>()
        );
    }
}

fn get_id(v: &Video) -> Option<Option<String>> {
    v.url.split("/").collect::<Vec<&str>>().last().map(|page| {
        page.split("?")
            .collect::<Vec<&str>>()
            .first()
            .map(|s| s.to_string())
    })
}

fn read_command_output(command: &mut Command, binary: &String) {
    match command.stdout(Stdio::piped()).spawn() {
        Ok(spawn) => match spawn.stdout {
            Some(stdout) => {
                for byte in stdout.bytes() {
                    print!("{}", byte.unwrap() as char);
                    io::stdout().flush().unwrap();
                }
            }
            None => (),
        },
        Err(e) => {
            if let NotFound = e.kind() {
                println!("`{}` was not found: maybe you should install it ?", binary)
            } else {
                println!("error while runnnig {} : {}", binary, e);
            }
            pause();
        }
    }
}

fn play_video(path: &String, app_config: &AppConfig) {
    for player in &app_config.players {
        if fs::metadata(&player[0]).is_ok() {
            let mut child1 = Command::new(&player[0]);
            for i in 1..player.len() {
                child1.arg(&player[i]);
            }
            read_command_output(child1.arg(path), &player[0]);
            return;
        }
    }
}

fn download_video(path: &String, id: &String, app_config: &AppConfig) {
    if !fs::metadata(&path).is_ok() {
        read_command_output(
            Command::new("youtube-dl")
                .arg("-f")
                .arg(&app_config.youtubedl_format)
                .arg("-o")
                .arg(&path)
                .arg("--")
                .arg(&id),
            &"youtube-dl".to_string(),
        )
    }
}

fn play_id(id: &String, app_config: &AppConfig) {
    if app_config.mpv_mode && fs::metadata(&app_config.mpv_path).is_ok() {
        let url = format!("https://www.youtube.com/watch?v={}", id);
        let message = format!("playing {} with mpv...", url);
        debug(&message);
        read_command_output(
            Command::new(&app_config.mpv_path)
            .arg("-fs")
            .arg("-really-quiet")
            .arg("--ytdl-format")
            .arg(&app_config.youtubedl_format)
            .arg(url)
            , &app_config.mpv_path);
    } else {
        clear();
        move_cursor(0);
        let path = format!(
            "{}/{}.{}",
            app_config.video_path, id, app_config.video_extension
        );
        download_video(&path, &id, app_config);
        play_video(&path, app_config);
    }
}

fn download_id(id: &String, app_config: &AppConfig) {
    clear();
    move_cursor(0);
    let path = format!(
        "{}/{}.{}",
        app_config.video_path, id, app_config.video_extension
    );
    download_video(&path, &id, app_config);
}

fn play(v: &Video, app_config: &AppConfig, download_only: bool) {
    match get_id(v) {
        Some(Some(id)) => {
            if download_only {
                download_id(&id, app_config)
            } else {
                play_id(&id, app_config);
            }
            ()
        }
        _ => (),
    }
}

fn print_help() {
    println!(
        "
  youtube-subscriptions: a tool to view your youtube subscriptions in a terminal

  q          quit
  j,l,down   move down
  k,up       move up
  g,H        go to top
  G,L        go to bottom
  M          go to middle
  r,$,left   soft refresh
  P          previous page
  N          next page
  R          full refresh (fetches video list)
  h,?        prints this help
  i,right    prints video information
  /          search
  f          filter
  p,enter    plays selected video
  o          open selected video in browser
  "
    )
}

fn print_info(v: &Video) {
    println!("{}", v.title);
    println!("");
    println!("from {}", v.channel);
    println!("");
    println!("{}", v.description);
}

fn quit() {
    show_cursor();
    rmcup();
}

impl YoutubeSubscribtions {
    fn clear_and_print_videos(&mut self) {
        clear();
        move_cursor(0);
        print_videos(&self.toshow)
    }

    fn move_page(&mut self, direction: i8) {
        self.n = get_lines();
        if direction == 1 {
            if self.start + 2 * self.n < self.videos.videos.len() {
                self.start += self.n;
            }
        } else if direction == 0 {
            self.start = 0;
        } else if direction == -1 {
            if self.n > self.start {
                self.start = 0;
            } else {
                self.start = self.start - self.n;
            }
        }
        self.toshow = to_show_videos(
            &mut self.videos.videos,
            self.start,
            self.start + self.n,
            &self.filter,
        );
        self.i = 0;
        self.clear_and_print_videos()
    }

    fn next_page(&mut self) {
        self.move_page(-1);
    }

    fn previous_page(&mut self) {
        self.move_page(1);
    }

    fn soft_reload(&mut self) {
        self.move_page(0);
    }

    fn hard_reload(&mut self) {
        debug(&"updating video list...".to_string());
        self.videos = load(true, &self.app_config).unwrap();
        debug(&"".to_string());
        self.soft_reload();
    }

    fn first_page(&mut self) {
        self.n = get_lines();
        self.toshow = to_show_videos(&mut self.videos.videos, self.start, self.n, &self.filter);
    }

    fn play_current(&mut self) {
        if self.i < self.toshow.len() {
            play(&self.toshow[self.i], &self.app_config, false);
            self.clear_and_print_videos();
        }
    }

    fn download_current(&mut self) {
        if self.i < self.toshow.len() {
            play(&self.toshow[self.i], &self.app_config, true);
            self.clear_and_print_videos();
        }
    }

    fn open_current(&mut self) {
        if self.i < self.toshow.len() {
            let url = &self.toshow[self.i].url;
            debug(&format!("opening {}", &url));
            let _res = webbrowser::open(&url);
        }
    }

    fn find(&mut self, s: String) -> usize {
        for (i, video) in self.toshow.iter().enumerate() {
            if video.channel.contains(s.as_str()) || video.title.contains(s.as_str()) {
                return i;
            }
        }
        0
    }

    fn input_with_prefix(&mut self, start_symbol: &str) -> String {
        move_to_bottom();
        print!("{}", start_symbol);
        io::stdout().flush().unwrap();
        let input = input();
        input.read_line().unwrap()
    }

    fn search(&mut self) {
        let s = self.input_with_prefix("/");
        self.i = self.find(s);
        self.clear_and_print_videos()
    }

    fn filter(&mut self) {
        let s = self.input_with_prefix("|");
        self.filter = s;
        self.move_page(0);
        self.clear_and_print_videos()
    }

    fn command(&mut self) {
        let s = self.input_with_prefix(":");
        let s = s.split_whitespace().collect::<Vec<&str>>();
        hide_cursor();
        clear();
        if s.len() == 2 {
            match s[0] {
                "o" => play_id(&s[1].to_string(), &self.app_config),
                _ => (),
            }
        }
        self.clear_and_print_videos()
    }

    fn wait_key_press_and_soft_reload(&mut self) {
        pause();
        clear();
        self.soft_reload();
    }

    fn info(&mut self) {
        if self.i < self.toshow.len() {
            clear();
            print_info(&self.toshow[self.i]);
            self.wait_key_press_and_soft_reload()
        }
    }

    fn help(&mut self) {
        clear();
        print_help();
        self.wait_key_press_and_soft_reload()
    }

    fn download(&mut self, take: usize) {
        self.hard_reload();
        for video in self.videos.videos.iter().rev().take(take) {
            match get_id(video) {
                Some(Some(id)) => {
                    let path = format!("/tmp/{}.mp4", id);
                    download_video(&path, &id, &self.app_config);
                }
                _ => (),
            }
        }
    }

    fn run(&mut self) {
        self.videos = load(false, &self.app_config).unwrap();
        self.start = 0;
        self.i = 0;
        smcup();
        self.first_page();
        self.clear_and_print_videos();
        hide_cursor();
        loop {
            print_selector(self.i);
            let input = input();
            let result;
            {
                let _screen = RawScreen::into_raw_mode();
                let mut stdin = input.read_sync();
                result = stdin.next();
            }
            match result {
                Some(key_event) => match key_event {
                    InputEvent::Keyboard(event) => match event {
                        Char('q') => {
                            quit();
                            break;
                        }
                        Char('j') | Char('l') | Down => self.i = jump(self.i, self.i + 1),
                        Char('k') | Up => {
                            self.i = jump(self.i, if self.i > 0 { self.i - 1 } else { self.n - 1 })
                        }
                        Char('g') | Char('H') => self.i = jump(self.i, 0),
                        Char('M') => self.i = jump(self.i, self.n / 2),
                        Char('G') | Char('L') => self.i = jump(self.i, self.n - 1),
                        Char('r') | Char('$') | Left => self.soft_reload(),
                        Char('P') => self.previous_page(),
                        Char('N') => self.next_page(),
                        Char('R') => self.hard_reload(),
                        Char('h') | Char('?') => self.help(),
                        Char('i') | Right => self.info(),
                        Char('p') | Char('\n') => self.play_current(),
                        Char('d') => self.download_current(),
                        Char('o') => self.open_current(),
                        Char('/') => self.search(),
                        Char(':') => self.command(),
                        Char('f') => self.filter(),
                        _ => debug(&format!("key not supported (press h for help)")),
                    },
                    _ => (),
                },
                _ => (),
            }
            self.i = self.i % self.n;
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut yts = YoutubeSubscribtions {
        n: 0,
        start: 0,
        filter: "".to_string(),
        i: 0,
        toshow: vec![],
        videos: Videos { videos: vec![] },
        app_config: load_config(),
    };
    match args.len() {
        2 => {
            match args[1].parse::<usize>() {
                Ok(_n) => yts.download(_n),
                Err(_) => yts.run(),
            };
        }
        _ => yts.run(),
    }
}
