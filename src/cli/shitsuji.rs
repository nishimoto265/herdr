use crate::api::schema::{
    Method, Request, RuleProposalListParams, RuleProposalStatus, RuleProposalSubmitParams,
    ShitsujiBackendProfileId,
};

pub(super) fn run_shitsuji_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("submit") => submit(&args[1..]),
        Some("list") => list(&args[1..]),
        Some("help" | "--help" | "-h") => {
            print_help();
            Ok(0)
        }
        _ => {
            print_help();
            Ok(2)
        }
    }
}

fn submit(args: &[String]) -> std::io::Result<i32> {
    let mut rule_text = None;
    let mut target_profile_id = None;
    let mut fingerprint = None;
    let mut source_event_id = None;
    let mut index = 0;
    while index < args.len() {
        let (slot, flag): (&mut Option<String>, &str) = match args[index].as_str() {
            "--rule" => (&mut rule_text, "--rule"),
            "--target" => (&mut target_profile_id, "--target"),
            "--fingerprint" => (&mut fingerprint, "--fingerprint"),
            "--source-event" => (&mut source_event_id, "--source-event"),
            "help" | "--help" | "-h" => {
                print_submit_help();
                return Ok(0);
            }
            other => {
                eprintln!("unknown shitsuji submit option: {other}");
                print_submit_help();
                return Ok(2);
            }
        };
        let Some(value) = args.get(index + 1) else {
            eprintln!("missing value for {flag}");
            return Ok(2);
        };
        *slot = Some(value.clone());
        index += 2;
    }

    let Some(rule_text) = rule_text else {
        eprintln!("missing required --rule");
        return Ok(2);
    };
    let Some(target_profile_id) = target_profile_id else {
        eprintln!("missing required --target");
        return Ok(2);
    };
    let Some(fingerprint) = fingerprint else {
        eprintln!("missing required --fingerprint");
        return Ok(2);
    };
    let Some(source_event_id) = source_event_id else {
        eprintln!("missing required --source-event");
        return Ok(2);
    };

    super::print_response(&super::send_request(&Request {
        id: "cli:shitsuji:submit".into(),
        method: Method::ShitsujiRuleProposalSubmit(RuleProposalSubmitParams {
            rule_text,
            target_profile_id: ShitsujiBackendProfileId::new(target_profile_id),
            fingerprint,
            source_event_id,
        }),
    })?)
}

fn list(args: &[String]) -> std::io::Result<i32> {
    let status = match args {
        [] => None,
        [flag, value] if flag == "--status" => match parse_status(value) {
            Some(status) => Some(status),
            None => {
                eprintln!("invalid status: {value}");
                return Ok(2);
            }
        },
        [flag] if matches!(flag.as_str(), "help" | "--help" | "-h") => {
            print_list_help();
            return Ok(0);
        }
        _ => {
            print_list_help();
            return Ok(2);
        }
    };
    super::print_response(&super::send_request(&Request {
        id: "cli:shitsuji:list".into(),
        method: Method::ShitsujiRuleProposalList(RuleProposalListParams { status }),
    })?)
}

fn parse_status(value: &str) -> Option<RuleProposalStatus> {
    match value {
        "pending" => Some(RuleProposalStatus::Pending),
        "approved" => Some(RuleProposalStatus::Approved),
        "rejected" => Some(RuleProposalStatus::Rejected),
        _ => None,
    }
}

fn print_help() {
    eprintln!("herdr shitsuji commands:");
    eprintln!("  herdr shitsuji submit --rule TEXT --target ID --fingerprint ID --source-event ID");
    eprintln!("  herdr shitsuji list [--status pending|approved|rejected]");
}

fn print_submit_help() {
    eprintln!(
        "usage: herdr shitsuji submit --rule TEXT --target ID --fingerprint ID --source-event ID"
    );
}

fn print_list_help() {
    eprintln!("usage: herdr shitsuji list [--status pending|approved|rejected]");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parser_accepts_only_public_status_names() {
        assert_eq!(parse_status("pending"), Some(RuleProposalStatus::Pending));
        assert_eq!(parse_status("approved"), Some(RuleProposalStatus::Approved));
        assert_eq!(parse_status("rejected"), Some(RuleProposalStatus::Rejected));
        assert_eq!(parse_status("approve"), None);
    }
}
