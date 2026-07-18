use clap::arg;

#[cfg(any(feature = "updating_cfg", feature = "using_api"))]
use clap::Arg;

#[cfg(feature = "updating_cfg")]
use {clap::value_parser, std::path::PathBuf};

pub fn build_args() -> clap::ArgMatches {
    let mut app = clap::Command::new("Yggdrasil peers updater")
        .version(env!("CARGO_PKG_VERSION"))
        .author("YggUser (https://matrix.to/#/@ygguser:matrix.org)");

    if cfg!(all(
        feature = "updating_cfg",
        feature = "using_api",
        feature = "self_updating"
    )) {
        app = app.about("The Yggdrasil peers updater automatically updates the peers in the Yggdrasil configuration file and/or calls addPeer/removePeer from the Yggdrasil Admin API.{n}Source code: https://github.com/ymeister/peers_updater")
    } else {
        app = app.about("The Yggdrasil peers updater automatically updates the peers in the Yggdrasil configuration file and/or calls addPeer/removePeer from the Yggdrasil Admin API.{n}Source code: https://github.com/ymeister/peers_updater{n}{n}!!! THE APPLICATION IS BUILT WITH THE `--no-default-features` OPTION!!!")
    }

    app = app.arg(
        arg!(
            -p --print "Print the peers sorted by latency. When using this parameter, all other parameters will be ignored."
        )
        .required(false)
    );

    #[cfg(feature = "updating_cfg")]
    {
        app = app.arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .default_value(crate::defaults::DEF_CFG_PATH)
                .value_name("FILE")
                .help("The path to the Yggdrasil configuration file (used when updating the configuration with `-u`)")
                .required(false)
                .value_parser(value_parser!(PathBuf)),
        );
    }

    #[cfg(feature = "updating_cfg")]
    {
        app = app.arg(
            arg!(
                -u --update_cfg "Make changes to the Yggdrasil configuration file. If not specified, no changes will be made to the file."
            )
            .required(false)
        )
        .arg(
            arg!(
                -r --restart "Restart the Yggdrasil (systemd or windows) service (effective only together with `-u`)"
            )
            .required(false)
        );
    }

    #[cfg(feature = "using_api")]
    {
        app = app.arg(
            arg!(
                -a --api "Add/remove peers during execution (requires enabling the admin API)"
            )
            .required(false),
        )
        .arg(
            Arg::new("socket")
                .short('s')
                .long("socket")
                .default_value(crate::defaults::DEF_SOCKET_ADDR)
                .value_name("URI")
                .help("The address of the Yggdrasil admin API socket (e.g. unix:///run/yggdrasil/yggdrasil.sock or tcp://localhost:9001)")
                .required(false),
        );
    }

    #[cfg(any(feature = "updating_cfg", feature = "using_api"))]
    {
        app = app.arg(
        arg!(
            -n --number <VALUE> "The number of peers to add (excluding extra ones)"
        )
        .required(false)
        .default_value("3")
    )
        .arg(
            arg!(
                -e --extra <VALUE> "A space-separated string with the URIs of the peers that should always be in the configuration"
            )
            .required(false));
    }

    app = app.arg(
        arg!(
            -i --ignore <VALUE> "A space-separated string of characters. Peers whose URIs contain combinations of this characters will not be added to the configuration"
        )
        .required(false))
    .arg(
        arg!(
            -I --ignore_country <VALUE> "A space-separated string containing the names of countries that will not be added to the configuration"
        )
        .required(false))
    ;

    #[cfg(feature = "self_updating")]
    {
        app = app.arg(
            arg!(
                -S --self_update "Self-updating of this utility. An executable file will be downloaded from the releases on GitHub (if a newer version is published there) and the current one will be replaced with a new one."
            )
            .required(false)
        );
    }

    app.get_matches()
}
