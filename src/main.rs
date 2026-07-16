use crate::peer::Peer;

use std::fs;
use std::path::PathBuf;
use std::process;

#[cfg(feature = "updating_cfg")]
mod cfg_file_read_write;

#[cfg(any(
    feature = "updating_cfg",
    feature = "using_api",
    feature = "self_updating"
))]
mod defaults;

#[cfg(feature = "using_api")]
mod using_api;

#[cfg(feature = "self_updating")]
mod self_updating;

mod clap_args;
mod download_file;
mod parsing_peers;
mod peer;
mod tmpdir;
mod unpack;

fn main() {
    let matches = clap_args::build_args();

    #[cfg(feature = "self_updating")]
    if matches.get_flag("self_update") {
        self_updating::self_update();
        process::exit(0);
    }

    let print_only = matches.get_flag("print");

    let update_cfg = if cfg!(feature = "updating_cfg") {
        matches.get_flag("update_cfg")
    } else {
        false
    };

    let use_api = if cfg!(feature = "using_api") {
        matches.get_flag("api")
    } else {
        false
    };

    #[cfg(feature = "using_api")]
    if !use_api && matches.value_source("socket") == Some(clap::parser::ValueSource::CommandLine) {
        eprintln!("Warning: the `-s` (`--socket`) option is ignored without `-a` (`--api`).");
    }

    #[cfg(all(feature = "updating_cfg", feature = "using_api"))]
    if use_api
        && !update_cfg
        && matches.value_source("config") == Some(clap::parser::ValueSource::CommandLine)
    {
        eprintln!("Warning: the configuration file is not used in API mode; the admin socket address is taken from `-s` (`--socket`).");
    }

    if !(print_only || update_cfg || use_api) {
        println!("At least the `-p` option is expected.");
        println!("For more information try '-h'.");
        println!("Nothing to do, exit.");
        process::exit(0);
    }

    #[cfg(feature = "updating_cfg")]
    let conf_path = match matches.get_one::<PathBuf>("config") {
        Some(conf_path) => conf_path,
        _ => {
            eprintln!("Can't get the configuration file default path.");
            process::exit(1);
        }
    };

    #[cfg(feature = "updating_cfg")]
    if update_cfg {
        // Checking if the file exists
        if !conf_path.exists() {
            eprintln!("The Yggdrasil configuration file does not exist.");
            process::exit(1);
        }

        // Checking write access to the configuration file
        if let Err(e) = check_permissions(conf_path) {
            eprintln!(
                "There is no write access to the Yggdrasil configuration file ({}).",
                e
            );
            process::exit(1);
        }
    }

    #[cfg(any(feature = "updating_cfg", feature = "using_api"))]
    let n_peers: u8 = if print_only {
        // With `-p` all other parameters are ignored
        0
    } else {
        let number = match matches.get_one::<String>("number") {
            Some(_n) => _n,
            _ => {
                eprintln!("Can't get the default number of peers.");
                process::exit(1);
            }
        };
        match number.parse() {
            Ok(_n) => _n,
            Err(e) => {
                eprintln!(
                    "The number of peers must be in the range from 0 to 255 ({}).",
                    e
                );
                process::exit(1);
            }
        }
    };

    #[cfg(any(feature = "updating_cfg", feature = "using_api"))]
    let extra_peers: Option<&String> = matches.get_one("extra");

    #[cfg(feature = "using_api")]
    let socket_addr = match matches.get_one::<String>("socket") {
        Some(_s) => _s,
        _ => {
            eprintln!("Can't get the admin socket default address.");
            process::exit(1);
        }
    };

    let ignored_peers: &str = match matches.get_one::<String>("ignore") {
        Some(_i_p) => _i_p.as_str(),
        None => "",
    };

    let ignored_countries: &str = match matches.get_one::<String>("ignore_country") {
        Some(_i_c) => _i_c.as_str(),
        None => "",
    };

    // The connected peers can only be checked against the ignore filters
    // after downloading the peers list, so with the filters the quick check
    // is skipped and a full reconciliation is forced. The same with `-u`:
    // the fresh list is downloaded anyway and is written to the
    // configuration file, so the daemon is reconciled to it too
    #[cfg(feature = "using_api")]
    let force_reconcile =
        !(ignored_peers.is_empty() && ignored_countries.is_empty()) || update_cfg;

    // In API-only mode `-n 0` needs nothing from the peers list: the
    // managed peers are removed (and the extras added) via the API alone
    #[cfg(feature = "using_api")]
    if use_api && !update_cfg && !print_only && n_peers == 0 {
        if !using_api::update_peers(&[], socket_addr, n_peers, extra_peers, force_reconcile) {
            process::exit(1);
        }
        process::exit(0);
    }

    // In API-only mode, first check whether there is anything to do at all,
    // so as not to download and ping peers for nothing
    #[cfg(feature = "using_api")]
    if use_api && !print_only && !force_reconcile {
        match using_api::quick_check(socket_addr, n_peers, extra_peers) {
            using_api::QuickCheck::Satisfied { extras_added } => {
                if extras_added > 0 {
                    println!("Added {} missing extra peer(s).", extras_added);
                }
                println!(
                    "Already connected to {} healthy public peers, nothing else to do.",
                    n_peers
                );
                process::exit(0);
            }
            using_api::QuickCheck::Error => {
                process::exit(1);
            }
            using_api::QuickCheck::NeedsWork | using_api::QuickCheck::AddOnly => {}
        }
    }

    // Creating a temporary directory
    let tmp_dir = match tmpdir::create_tmp_dir(None) {
        Ok(val) => val,
        Err(e) => {
            eprintln!("Failed to create a temporary directory ({}).", e);
            process::exit(1);
        }
    };

    // Download the archive with peers
    if let Err(e) = download_file::download_archive(
        "https://github.com/yggdrasil-network/public-peers/archive/refs/heads/master.zip",
        &tmp_dir,
        "peers.zip",
    ) {
        eprintln!("Failed to download archive with peers ({}).", e);
        process::exit(1);
    }

    // Unpacking the downloaded archive
    //let tmp_dir_ = PathBuf::from("/home/user/rust/peers_updater/target/debug/"); //test
    if let Err(e) = crate::unpack::unpack_archive(&tmp_dir, "peers.zip") {
        eprintln!("Failed to unpack archive ({}).", e);
        process::exit(1);
    }

    // Deleting unnecessary files
    let _ = fs::remove_file(std::path::Path::new(
        format!("{}/public-peers-master/README.md", &tmp_dir.display()).as_str(),
    ));
    let _ = fs::remove_file(std::path::Path::new(
        format!("{}/peers.zip", &tmp_dir.display()).as_str(),
    ));
    let _ = fs::remove_dir_all(std::path::Path::new(
        format!("{}/public-peers-master/other", &tmp_dir.display()).as_str(),
    ));

    let peers_dir: PathBuf =
        std::path::Path::new(format!("{}/public-peers-master/", &tmp_dir.display()).as_str())
            .to_path_buf();

    // Collecting peers in a vector
    let mut peers: Vec<Peer> = Vec::new();
    if let Err(e) = crate::parsing_peers::collect_peers(
        &peers_dir,
        &mut peers,
        ignored_peers,
        ignored_countries,
    ) {
        eprintln!("Couldn't get peer addresses from downloaded files ({}).", e);
        process::exit(1);
    };

    // Deleting unnecessary files
    let _ = fs::remove_dir_all(std::path::Path::new(tmp_dir.as_path()));

    // Calculating latency
    std::thread::scope(|scope| {
        for peer in &mut peers {
            scope.spawn(move || {
                peer.set_latency();
            });
        }
    });

    //Sorting the vector
    peers.sort_by(|a, b| a.latency.cmp(&b.latency));

    // Printing data
    if print_only {
        println!(
            "{0:<60}|{1:<15}|{2:<15}|{3:<10}",
            "URI", "Region", "Country", "Latency"
        );
        println!("{0:-<100}", "-");
        for peer in peers {
            if !peer.is_alive {
                break;
            }
            println!(
                "{0:<60}|{1:<15}|{2:<15}|{3:<10}",
                peer.uri, peer.region, peer.country, peer.latency
            );
        }
        process::exit(0);
    } else if update_cfg || use_api {
        // Adding peers to the configuration file
        #[cfg(feature = "updating_cfg")]
        if update_cfg {
            //Reading the configuration file
            let cfg_txt = match cfg_file_read_write::read_config(conf_path) {
                Ok(_ct) => _ct,
                Err(e) => {
                    eprintln!("The configuration file cannot be read ({}).", e);
                    process::exit(1);
                }
            };

            cfg_file_read_write::add_peers_to_conf_new(
                &peers,
                conf_path,
                n_peers,
                extra_peers,
                &cfg_txt,
            );
        }

        //Restart if required
        #[cfg(feature = "updating_cfg")]
        if update_cfg && matches.get_flag("restart") {
            #[cfg(not(target_os = "windows"))]
            let _ = std::process::Command::new("systemctl")
                .arg("restart")
                .arg("yggdrasil")
                .spawn();

            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("net")
                    .arg("stop")
                    .arg("yggdrasil")
                    .output();
                let _ = std::process::Command::new("net")
                    .arg("start")
                    .arg("yggdrasil")
                    .spawn();
            }
        }

        // Adding/removing peers during execution
        #[cfg(feature = "using_api")]
        if use_api
            && !using_api::update_peers(&peers, socket_addr, n_peers, extra_peers, force_reconcile)
        {
            process::exit(1);
        }
    }
}

#[cfg(feature = "updating_cfg")]
fn check_permissions(path: &PathBuf) -> std::io::Result<()> {
    // Opening for writing checks actual access rights, unlike the read-only
    // attribute, which does not take the current user into account.
    fs::OpenOptions::new().write(true).open(path)?;
    Ok(())
}
