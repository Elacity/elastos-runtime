//! /command parser for chat input.

/// Parsed chat command
pub enum Command {
    /// Join or create a channel
    Join(String),
    /// Leave current channel
    Part,
    /// Direct message
    Msg(String, String),
    /// Change nickname
    Nick(String),
    /// Connect to a peer via ticket
    Connect(String),
    /// Show your ticket for sharing
    Ticket,
    /// List connected peers
    Peers,
    /// List joined channels
    List,
    /// Show help
    Help,
    /// Return Home
    Home,
    /// Quit chat
    Quit,
    /// Regular message (not a command)
    Message(String),
    /// Local-only error/usage message (not broadcast)
    Error(String),
}

/// Parse input string into a Command.
pub fn parse(input: &str) -> Command {
    let input = input.trim();

    if !input.starts_with('/') {
        return Command::Message(input.to_string());
    }

    let parts: Vec<&str> = input.splitn(3, ' ').collect();
    let cmd = parts[0].to_lowercase();

    match cmd.as_str() {
        "/join" | "/j" => {
            if parts.len() < 2 {
                return Command::Error("Usage: /join #channel".to_string());
            }
            if parts[1].starts_with('@') {
                return Command::Error(
                    "Cannot /join @names. DM channels are created via /msg.".to_string(),
                );
            }
            let channel = if parts[1].starts_with('#') {
                parts[1].to_string()
            } else {
                format!("#{}", parts[1])
            };
            Command::Join(channel)
        }
        "/part" | "/leave" => Command::Part,
        "/msg" | "/dm" => {
            if parts.len() < 3 {
                return Command::Error("Usage: /msg nick message".to_string());
            }
            Command::Msg(parts[1].to_string(), parts[2].to_string())
        }
        "/nick" => {
            if parts.len() < 2 {
                return Command::Error("Usage: /nick name".to_string());
            }
            Command::Nick(parts[1].to_string())
        }
        "/connect" | "/c" => {
            if parts.len() < 2 {
                return Command::Error("Usage: /connect ticket".to_string());
            }
            Command::Connect(parts[1].to_string())
        }
        "/ticket" | "/t" => Command::Ticket,
        "/peers" | "/who" => Command::Peers,
        "/list" | "/ls" => Command::List,
        "/help" | "/h" | "/?" => Command::Help,
        "/home" => Command::Home,
        "/quit" | "/q" | "/exit" => Command::Quit,
        _ => Command::Error(format!("Unknown command: {}", parts[0])),
    }
}

pub fn launched_from_home() -> bool {
    std::env::var("ELASTOS_PARENT_SURFACE").ok().as_deref() == Some("home")
}

/// Help text for all commands.
pub fn help_text(from_home: bool) -> String {
    let home_line = if from_home {
        " /home            Return Home"
    } else {
        " /home            Exit chat to terminal"
    };
    let quit_line = if from_home {
        " /quit            Leave chat and return Home"
    } else {
        " /quit            Exit chat to terminal"
    };

    format!(
        "Channels:\n\
         \x20 /join #channel   Join or create a channel\n\
         \x20 /part            Leave current channel\n\
         \x20 /list            Show joined channels\n\
         \n\
         Messaging:\n\
         \x20 /msg nick text   Send a direct message\n\
         \x20 /nick name       Change your nickname\n\
         \n\
         Peers:\n\
         \x20 /peers           List connected peers\n\
         \x20 /ticket          Show your connect code\n\
         \x20 /connect <code>  Connect to someone's code\n\
         \n\
         Other:\n\
         \x20 /help            Show this help\n\
        {home_line}\n\
        {quit_line}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_message() {
        match parse("hello world") {
            Command::Message(m) => assert_eq!(m, "hello world"),
            _ => panic!("Expected message"),
        }
    }

    #[test]
    fn test_parse_join() {
        match parse("/join #general") {
            Command::Join(ch) => assert_eq!(ch, "#general"),
            _ => panic!("Expected join"),
        }
        // Without hash prefix
        match parse("/join general") {
            Command::Join(ch) => assert_eq!(ch, "#general"),
            _ => panic!("Expected join"),
        }
    }

    #[test]
    fn test_parse_quit() {
        assert!(matches!(parse("/quit"), Command::Quit));
        assert!(matches!(parse("/q"), Command::Quit));
        assert!(matches!(parse("/exit"), Command::Quit));
        assert!(matches!(parse("/home"), Command::Home));
    }

    #[test]
    fn test_parse_nick() {
        match parse("/nick alice") {
            Command::Nick(n) => assert_eq!(n, "alice"),
            _ => panic!("Expected nick"),
        }
    }

    #[test]
    fn test_parse_msg() {
        match parse("/msg bob hello there") {
            Command::Msg(to, text) => {
                assert_eq!(to, "bob");
                assert_eq!(text, "hello there");
            }
            _ => panic!("Expected msg"),
        }
    }

    #[test]
    fn test_parse_help() {
        assert!(matches!(parse("/help"), Command::Help));
        assert!(matches!(parse("/?"), Command::Help));
    }

    #[test]
    fn test_parse_join_rejects_at_prefix() {
        assert!(matches!(parse("/join @alice"), Command::Error(_)));
    }

    #[test]
    fn help_text_is_explicit_for_home_launch() {
        let help = help_text(true);
        assert!(help.contains("/home            Return Home"));
        assert!(help.contains("/quit            Leave chat and return Home"));
    }

    #[test]
    fn help_text_is_explicit_for_terminal_launch() {
        let help = help_text(false);
        assert!(help.contains("/home            Exit chat to terminal"));
        assert!(help.contains("/quit            Exit chat to terminal"));
    }

}
