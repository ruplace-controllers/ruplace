extern crate reqwest;
extern crate serde_json;
#[macro_use]
extern crate hyper;
#[macro_use]
extern crate serde_derive;
extern crate png;
extern crate rand;
extern crate rpassword;
extern crate clap;

use std::env;
use std::collections::HashMap;
use std::thread;
use std::time::Duration;
use std::error::Error;
use std::process;
use std::collections::VecDeque;
use std::rc::Rc;
use std::cell::RefCell;
use std::collections::HashSet;

use serde_json::Value;
use reqwest::{RequestBuilder, Client};
use hyper::header::Cookie;
use png::HasParameters;
use clap::{Arg, App, SubCommand};

header! { (XModhash, "x-modhash") => [String] }

#[derive(Debug)]
struct RedditSession {
    pub modhash: String,
    pub cookie: String
}

#[derive(Debug, Eq, PartialEq)]
struct TargetJson {
    pub major_version: u32,
    pub minor_version: u32,
    pub x: u32,
    pub y: u32,
    pub image: String,
    pub fallbacks: Vec<String>,
}

const TARGET_JSON_URL: &'static str = "https://raw.githubusercontent.com/ruplace-controllers/ruplace-target/master/ruplace.json";

const PALETTE: [[u8; 4]; 17] = [
    [255, 255, 255, 255],
    [228, 228, 228, 255],
    [136, 136, 136, 255],
    [ 34,  34,  34, 255],
    [255, 167, 209, 255],
    [229,   0,   0, 255],
    [229, 149,   0, 255],
    [160, 106,  66, 255],
    [229, 217,   0, 255],
    [148, 224,  68, 255],
    [  2, 190,   1, 255],
    [  0, 211, 221, 255],
    [  0, 131, 199, 255],
    [  0,   0, 234, 255],
    [207, 110, 228, 255],
    [130,   0, 128, 255],
    [  0,   0,   0,   0],
];

const MAJOR_VERSION: u32 = 1;
const MINOR_VERSION: u32 = 1;

const DEBUG: bool = false;

fn color_to_index(color: &[u8]) -> u8 {
    if color[3] < 128 {
        return 16;
    }
    PALETTE.iter().enumerate().map(|(index, p)| {
        (index, p.iter().zip(color.iter()).map(|(a, b)| {
            let diff = *a as i32 - *b as i32;
            diff*diff
        }).sum::<i32>())
    }).min_by_key(|&(_, diff)| diff).expect("4 components").0 as u8
}

fn get_target_json(url: &str) -> Result<TargetJson, Box<Error>> {
    let new_target: serde_json::Value = reqwest::get(url)?.json()?;
    let new_target = new_target.as_object().ok_or("Json format error")?;
    macro_rules! tr {
        ($e:expr) => {
            ($e).ok_or("Json format error")?
        }
    }

    let fallbacks = || -> Result<Vec<String>, Box<Error>> {
        Ok(tr!(tr!(tr!(new_target.get("fallbacks")).as_array())
            .iter()
            .map(|x| x.as_str().map(|x| x.to_string()))
            .collect::<Option<Vec<String>>>()))
    };
    let fallbacks = fallbacks().unwrap_or_default();

    let new_target = TargetJson {
        major_version: new_target.get("major_version")
                            .and_then(|x| x.as_u64())
                            .map(|x| x as u32).unwrap_or(MAJOR_VERSION),
        minor_version: new_target.get("minor_version")
                            .and_then(|x| x.as_u64())
                            .map(|x| x as u32).unwrap_or(MINOR_VERSION),
        x:             tr!(tr!(new_target.get("x")).as_u64()) as u32,
        y:             tr!(tr!(new_target.get("y")).as_u64()) as u32,
        image:         tr!(tr!(new_target.get("image")).as_str()).to_string(),
        fallbacks:     fallbacks,
    };

    if new_target.major_version > MAJOR_VERSION {
        println!("New major version is available. Must update!");
        process::exit(1);
    }

    if new_target.minor_version > MINOR_VERSION {
        println!("New minor version is available. Update when convenient.");
    }

    Ok(new_target)
}

struct Job {
    url: String,
    target: TargetJson,
    width: u32,
    height: u32,
    target_image: Vec<u8>,
    fallbacks: Vec<Rc<RefCell<Job>>>,
}

impl Job {
    fn new(url: String) -> Self {
        Job {
            url: url,
            target: TargetJson {
                major_version: MAJOR_VERSION,
                minor_version: MINOR_VERSION,
                x: 0,
                y: 0,
                image: String::new(),
                fallbacks: vec![],
            },
            width: 0,
            height: 0,
            target_image: Vec::new(),
            fallbacks: Vec::new(),
        }
    }
}

fn try_place_pixel(root: &RefCell<Job>,
                   mut board: &mut Vec<u8>,
                   username: &str,
                   password: &str) -> Result<(), Box<Error>> {
    let new_target = get_target_json(&root.borrow().url)?;

    println!("Target: {}", root.borrow().url);

    if new_target != root.borrow().target {
        let mut root = root.borrow_mut();
        let root = &mut*root;

        root.target = new_target;
        let mut decoder = png::Decoder::new(reqwest::get(&root.target.image)?);
        decoder.set(png::TRANSFORM_EXPAND | png::TRANSFORM_GRAY_TO_RGB | png::TRANSFORM_PACKING | png::TRANSFORM_STRIP_16);
        let (info, mut reader) = decoder.read_info()?;
        root.width = info.width;
        root.height = info.height;
        let mut buffer = Vec::new();
        buffer.resize(info.buffer_size(), 0u8);
        reader.next_frame(&mut *buffer)?;

        root.target_image.truncate(0);
        root.target_image.reserve_exact((root.width * root.height) as usize);

        root.fallbacks = root.target.fallbacks.iter()
            .map(|s| Job::new(s.to_string()))
            .map(|j| Rc::new(RefCell::new(j)))
            .collect();

        match info.color_type {
            png::ColorType::RGB => {
                for color in buffer.chunks(3) {
                    let c = [color[0], color[1], color[2], 255];
                    root.target_image.push(color_to_index(&c));
                }
            },
            png::ColorType::RGBA => {
                for color in buffer.chunks(4) {
                    root.target_image.push(color_to_index(color));
                }
            },
            _ => return Err("Reference image has unsupported color type".into())
        }
    }

    let root = root.borrow();

    if DEBUG {
        println!("{:?}", root.target);
    }

    fetch_board(&mut board)?;
    let (x, y, color) = pick_random_pixel(&board,
        root.target.x, root.target.y, root.width, root.height, &root.target_image)?;

    println!("  Attempting to place pixel: ({}, {}) - {}", x, y, color);

    let session = reddit_login(&username, &password)?;
    let delay = place_pixel(x, y, color, &session)?;

    println!("Sleeping for {} seconds...", delay);
    thread::sleep(Duration::from_secs(delay as u64));

    Ok(())
}

const TARGET_DONE: &'static str = "Nothing to do (for now)";

fn main() {
    let matches = App::new("ruplace")
        .arg(Arg::with_name("config")
            .short("c")
            .long("config")
            .value_name("URL")
            .help("Set a custom JSON config URL")
            .takes_value(true))
        .arg(Arg::with_name("username")
            .required(false)
            .index(1))
        .arg(Arg::with_name("password")
            .required(false)
            .index(2))
        .get_matches();

    let mut username = matches.value_of("username").map(|s| s.to_string());
    let mut password = matches.value_of("password").map(|s| s.to_string());

    if username.is_none() {
        println!("Enter username:");
        let mut s = "".into();
        std::io::stdin().read_line(&mut s).unwrap();
        username = Some(s);
    }

    if password.is_none() {
        println!("Enter password:");
        let s = rpassword::read_password().unwrap();
        password = Some(s);
    }

    let username = username.expect("<username> argument");
    let password = password.expect("<password> argument");

    let init_config = matches.value_of("config").unwrap_or(TARGET_JSON_URL);

    let mut board = Vec::new();
    board.resize(1000*1000/2, 0u8);

    let root = Rc::new(RefCell::new(Job::new(init_config.to_string())));

    loop {
        let mut queue = VecDeque::new();
        queue.push_front(root.clone());

        let mut already_seen = HashSet::new();

        while !queue.is_empty() {
            let root = queue.pop_front().unwrap();
            already_seen.insert(root.borrow().url.to_string());

            if let Err(e) = try_place_pixel(&root, &mut board, &username, &password) {
                let emsg = format!("{}", e);

                if emsg == TARGET_DONE {
                    for fb in &root.borrow().fallbacks {
                        let fb = fb.clone();
                        let url = fb.borrow().url.to_string();
                        if !already_seen.contains(&url) {
                            queue.push_back(fb);
                            already_seen.insert(url);
                        } else if DEBUG {
                            println!("  Skipping repeat target {}", url);
                        }
                    }
                } else {
                    queue.clear();
                }

                if queue.is_empty() {
                    println!("{} - sleeping for 10 seconds", emsg);
                    thread::sleep(Duration::from_secs(10));
                }
            } else {
                queue.clear();
            }
        }
        println!();
    }
}

fn sample_board(board: &[u8], x: u32, y: u32) -> u8 {
    let v = board[((y as usize))*500 + (x as usize)/2];
    if x % 2 == 0 {
        v >> 4
    } else {
        v & 0xF
    }
}

fn sample_target(target: &[u8], x: u32, y: u32, width: u32) -> u8 {
    target[(y as usize)*(width as usize) + (x as usize)]
}

fn pick_random_pixel(board: &[u8], x: u32, y: u32, width: u32, height: u32, target_image: &[u8])
                     -> Result<(u32, u32, u8), Box<Error>> {
    use rand::Rng;
    let mut count = 0;
    let mut solid = 0;
    let hex = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "a", "b", "c", "d", "e", "f", "."];
    for py in 0..height {
        let mut sb = String::new();
        let mut st = String::new();
        for px in 0..width {
            let bp = sample_board(board, x + px, y + py);
            let tp = sample_target(target_image, px, py, width);
            if DEBUG {
                sb += hex[bp as usize];
                st += hex[tp as usize];
            }
            if tp != 16 && tp != bp {
                count += 1;
            }
            if tp != 16 {
                solid += 1;
            }
        }
        if DEBUG {
            println!("{} - {}", sb, st);
        }
    }
    let done = solid - count;
    let percentage_done = ((done*1000/solid) as f64)*0.1;
    println!("  Progress: {}/{} ({:.1}%)", done, solid, percentage_done);

    if count == 0 || DEBUG {
        return Err(TARGET_DONE.into());
    }

    let mut index = rand::thread_rng().gen_range(0, count);
    for py in 0..height {
        for px in 0..width {
            let bp = sample_board(board, x + px, y + py);
            let tp = sample_target(target_image, px, py, width);
            if tp != 16 && tp != bp {
                index -= 1;
                if index == 0  {
                    return Ok((px + x, py + y, tp));
                }
            }
        }
    }

    Err(TARGET_DONE.into())
}

fn fetch_board(board: &mut Vec<u8>) -> Result<(), Box<Error>> {
    use std::io::Read;
    let mut file = reqwest::get("https://www.reddit.com/api/place/board-bitmap")?;
    file.read_exact(&mut board[0..4])?;
    file.read_exact(&mut *board)?;
    Ok(())
}

fn place_pixel(x: u32, y: u32, color: u8, session: &RedditSession) -> Result<u32, Box<Error>> {
    let client = Client::new()?;

    let mut params = HashMap::new();
    params.insert("x", x);
    params.insert("y", y);
    params.insert("color", color as u32);

    let response: Value = reddit_auth(client.post("https://www.reddit.com/api/place/draw.json"), session)
        .form(&params)
        .send()?
        .json()?;
    Ok(response.get("wait_seconds").and_then(Value::as_u64)
                                   .ok_or("Did not receive wait time")? as u32)
}

fn reddit_login(username: &str, password: &str) -> Result<RedditSession, Box<Error>> {
    let client = Client::new()?;

    let mut params = HashMap::new();
    params.insert("op", "login-main");
    params.insert("user", &username);
    params.insert("passwd", &password);
    params.insert("rem", "on");
    params.insert("api_type", "json");

    let response: Value = client.post(&format!("https://www.reddit.com/api/login/{}", username))
        .form(&params)
        .send()?
        .json()?;

    let inner = response.get("json").ok_or("No json returned from login")?;
    let errors = inner.get("errors").and_then(Value::as_array).ok_or("No errors returned from login")?;
    if errors.len() > 0 {
        return Err(format!("Login errors: {:?}", errors).into());
    }
    let data = inner.get("data").ok_or("No data returned from login")?;

    Ok(RedditSession {
        modhash: data.get("modhash").and_then(Value::as_str)
                                    .ok_or("No modhash returned from login")?.to_owned(),
        cookie: data.get("cookie").and_then(Value::as_str)
                                  .ok_or("No cookie returned from login")?.to_owned(),
    })
}

fn reddit_auth(req: RequestBuilder, session: &RedditSession) -> RequestBuilder {
    req
        .header(XModhash(session.modhash.clone()))
        .header(Cookie(vec![
            format!("reddit_session={}", session.cookie)
        ]))
}
