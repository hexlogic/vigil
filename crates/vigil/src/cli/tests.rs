//! Tests of the command-line definition.

use super::*;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("vigil").chain(args.iter().copied()))
}

#[test]
fn serve_can_turn_sign_in_off() -> Result<(), clap::Error> {
    assert!(matches!(
        parse(&["serve"])?.command,
        Command::Serve { no_auth: false }
    ));
    assert!(matches!(
        parse(&["serve", "--no-auth"])?.command,
        Command::Serve { no_auth: true }
    ));
    Ok(())
}

#[test]
fn serve_is_the_server_command() -> Result<(), clap::Error> {
    assert!(parse(&["serve"])?.command.is_server());
    assert!(!parse(&["watch", "list"])?.command.is_server());
    Ok(())
}

#[test]
fn watch_add_parses_url_interval_and_name() -> Result<(), clap::Error> {
    let cli = parse(&[
        "watch",
        "add",
        "https://example.com",
        "--every",
        "5m",
        "--name",
        "Ex",
    ])?;
    let Command::Watch {
        command: WatchCommand::Add {
            url, every, name, ..
        },
    } = cli.command
    else {
        panic!("expected watch add");
    };
    assert_eq!(url, "https://example.com");
    assert_eq!(every, Duration::from_secs(300));
    assert_eq!(name.as_deref(), Some("Ex"));
    Ok(())
}

#[test]
fn watch_add_defaults_to_hourly() -> Result<(), clap::Error> {
    let cli = parse(&["watch", "add", "https://example.com"])?;
    let Command::Watch {
        command: WatchCommand::Add { every, name, .. },
    } = cli.command
    else {
        panic!("expected watch add");
    };
    assert_eq!(every, Duration::from_secs(3600));
    assert_eq!(name, None);
    Ok(())
}

#[test]
fn bad_intervals_are_rejected() {
    assert!(parse(&["watch", "add", "https://example.com", "--every", "0s"]).is_err());
    assert!(parse(&["watch", "add", "https://example.com", "--every", "500ms"]).is_err());
    assert!(parse(&["watch", "add", "https://example.com", "--every", "soon"]).is_err());
}

#[test]
fn check_diff_and_remove_take_an_id() -> Result<(), clap::Error> {
    assert!(
        matches!(parse(&["check", "abc123"])?.command, Command::Check { id, .. } if id == "abc123")
    );
    assert!(matches!(parse(&["diff", "abc123"])?.command, Command::Diff { id } if id == "abc123"));
    let remove = parse(&["watch", "remove", "abc123"])?.command;
    assert!(matches!(
        remove,
        Command::Watch { command: WatchCommand::Remove { id } } if id == "abc123"
    ));
    Ok(())
}

#[test]
fn watch_add_takes_content_options() -> Result<(), clap::Error> {
    let cli = parse(&[
        "watch",
        "add",
        "https://example.com",
        "--processor",
        "json",
        "--pipeline",
        "[]",
        "--conditions",
        "@rules.json",
    ])?;
    let Command::Watch {
        command: WatchCommand::Add { content, .. },
    } = cli.command
    else {
        panic!("expected watch add");
    };
    assert_eq!(content.processor, Some(Processor::Json));
    assert_eq!(content.pipeline.as_deref(), Some("[]"));
    assert_eq!(content.conditions.as_deref(), Some("@rules.json"));
    assert!(!content.is_empty());
    Ok(())
}

#[test]
fn watch_add_without_content_options_has_none() -> Result<(), clap::Error> {
    let cli = parse(&["watch", "add", "https://example.com"])?;
    let Command::Watch {
        command: WatchCommand::Add { content, .. },
    } = cli.command
    else {
        panic!("expected watch add");
    };
    assert!(content.is_empty());
    Ok(())
}

#[test]
fn unknown_processors_are_rejected() {
    assert!(parse(&["watch", "add", "https://example.com", "--processor", "xml"]).is_err());
}

#[test]
fn watch_edit_changes_only_what_it_is_given() -> Result<(), clap::Error> {
    let cli = parse(&["watch", "edit", "abc123", "--every", "10m", "--name", "N"])?;
    let Command::Watch {
        command:
            WatchCommand::Edit {
                id,
                every,
                name,
                content,
                ..
            },
    } = cli.command
    else {
        panic!("expected watch edit");
    };
    assert_eq!(id, "abc123");
    assert_eq!(every, Some(Duration::from_secs(600)));
    assert_eq!(name.as_deref(), Some("N"));
    assert!(content.is_empty());
    assert!(parse(&["watch", "edit", "abc123", "--every", "0s"]).is_err());
    assert!(parse(&["watch", "edit"]).is_err());
    Ok(())
}

#[test]
fn watch_notification_options_repeat() -> Result<(), clap::Error> {
    let cli = parse(&[
        "watch",
        "add",
        "https://example.com",
        "--notify",
        "phone",
        "--notify",
        "chat",
        "--notify-template",
        "{{ watch_name }}",
    ])?;
    let Command::Watch {
        command: WatchCommand::Add { notify, .. },
    } = cli.command
    else {
        panic!("expected watch add");
    };
    assert_eq!(notify.notify, ["phone", "chat"]);
    assert_eq!(notify.notify_template.as_deref(), Some("{{ watch_name }}"));

    let cli = parse(&[
        "watch",
        "edit",
        "abc123",
        "--no-notify",
        "phone",
        "--notify",
        "chat",
    ])?;
    let Command::Watch {
        command: WatchCommand::Edit {
            notify, no_notify, ..
        },
    } = cli.command
    else {
        panic!("expected watch edit");
    };
    assert_eq!(
        (notify.notify, no_notify),
        (vec!["chat".to_owned()], vec!["phone".to_owned()])
    );
    Ok(())
}

#[test]
fn check_can_skip_notifications() -> Result<(), clap::Error> {
    assert!(matches!(
        parse(&["check", "abc123", "--no-notify"])?.command,
        Command::Check {
            no_notify: true,
            ..
        }
    ));
    assert!(matches!(
        parse(&["check", "abc123"])?.command,
        Command::Check {
            no_notify: false,
            ..
        }
    ));
    Ok(())
}

#[test]
fn preview_takes_an_id_and_optional_overrides() -> Result<(), clap::Error> {
    let cli = parse(&["preview", "abc123", "--pipeline", "[]"])?;
    let Command::Preview { id, content } = cli.command else {
        panic!("expected preview");
    };
    assert_eq!(id, "abc123");
    assert_eq!(content.pipeline.as_deref(), Some("[]"));
    assert!(parse(&["preview"]).is_err());
    Ok(())
}

#[test]
fn watch_show_takes_an_id() -> Result<(), clap::Error> {
    assert!(matches!(
        parse(&["watch", "show", "abc123"])?.command,
        Command::Watch { command: WatchCommand::Show { id } } if id == "abc123"
    ));
    Ok(())
}

#[test]
fn missing_arguments_are_errors() {
    assert!(parse(&["check"]).is_err());
    assert!(parse(&["watch", "add"]).is_err());
    assert!(parse(&[]).is_err());
}
