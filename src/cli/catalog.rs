//! CLI discovery and argument validation share this public command catalog.
use crate::{
    Result,
    export::Format,
    pipeline::{self, ModelVariant},
    protocol,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
};

pub(super) const DEFAULT_RESPONSE: &str = "compact";
pub(super) const RESPONSES: &[&str] = &[DEFAULT_RESPONSE, "metadata", "full"];
pub(super) const LEGACY_GIGASTT: &str = "gigastt";
pub(super) const LEGACY_MODELS_DIR: &str = ".gigastt/models";
pub(super) const DOCTOR_ROOT: &str = "runs/doctor";

#[derive(Clone, Copy)]
enum Kind {
    Path,
    Switch,
    UInt {
        min: u64,
        max: u64,
        default: Option<u64>,
    },
    Timestamp,
    Response,
    Model,
    Format,
}
#[derive(Clone, Copy)]
pub(super) struct Parameter {
    kind: Kind,
}
impl Parameter {
    pub(super) fn switch(self) -> bool {
        matches!(self.kind, Kind::Switch)
    }
}
pub(super) fn parameter(name: &str) -> Option<Parameter> {
    use Kind::*;
    Some(Parameter {
        kind: match name {
            "--work-dir" | "--model-dir" | "--runtime-dir" | "--archive" | "--gigastt"
            | "--output" => Path,
            "--force" | "--offline" => Switch,
            "--chunk-seconds" => UInt {
                min: u64::from(pipeline::CHUNK_SECONDS_MIN),
                max: u64::from(pipeline::CHUNK_SECONDS_MAX),
                default: Some(300),
            },
            "--asr-timeout-seconds" => UInt {
                min: 1,
                max: u64::MAX,
                default: Some(1800),
            },
            "--retry-window-seconds" => UInt {
                min: 0,
                max: u64::MAX,
                default: Some(3600),
            },
            "--asr-memory-mib" => UInt {
                min: 1,
                max: u64::MAX / (1024 * 1024),
                default: None,
            },
            "--boundary-window-ms" => UInt {
                min: 0,
                max: u64::from(pipeline::BOUNDARY_WINDOW_MAX_MS),
                default: Some(0),
            },
            "--encoder-threads" => UInt {
                min: 1,
                max: usize::MAX as u64,
                default: None,
            },
            "--jobs" => UInt {
                min: 1,
                max: crate::batch::MAX_WORKERS as u64,
                default: Some(1),
            },
            "--start" | "--end" => Timestamp,
            "--response" => Response,
            "--model-variant" => Model,
            "--format" => Format,
            _ => return None,
        },
    })
}
pub(super) fn number_default(name: &str) -> Result<u64> {
    match parameter(name).map(|p| p.kind) {
        Some(Kind::UInt {
            default: Some(n), ..
        }) => Ok(n),
        _ => Err("missing numeric CLI default".into()),
    }
}
pub(super) fn response_default() -> &'static str {
    DEFAULT_RESPONSE
}
pub(super) fn response_envelope_version(response: &OsStr) -> u32 {
    if response == "full" { 1 } else { 2 }
}
pub(super) fn validate_value(name: &str, value: &OsStr) -> Result<()> {
    let parameter = parameter(name).ok_or("unknown CLI parameter")?;
    if matches!(parameter.kind, Kind::Path) {
        return Ok(());
    }
    let text = value.to_str().ok_or("option value must be UTF-8")?;
    let valid = match parameter.kind {
        Kind::Path => true,
        Kind::Switch => text == "true",
        Kind::UInt { min, max, .. } => text.parse::<u64>().is_ok_and(|n| (min..=max).contains(&n)),
        Kind::Timestamp => {
            crate::time_range::timestamp(text)?;
            true
        }
        Kind::Response => RESPONSES.contains(&text),
        Kind::Model => text.parse::<ModelVariant>().is_ok(),
        Kind::Format => text.parse::<Format>().is_ok(),
    };
    if !valid {
        return Err(protocol::failure(
            if name == "--jobs" {
                "batch_workers_invalid"
            } else {
                "invalid_arguments"
            },
            Some(false),
            format!("invalid value for {name}; use describe for accepted values"),
        ));
    }
    Ok(())
}
struct Command {
    name: &'static str,
    token: &'static str,
    aliases: &'static [&'static str],
    positional: Option<(&'static str, bool)>,
    flags: &'static [&'static str],
    required: &'static [&'static str],
    outputs: &'static [&'static str],
}
const COMMANDS: &[Command] = &[
    Command {
        name: "transcribe",
        token: "transcribe",
        aliases: &[],
        positional: Some(("url", true)),
        flags: &[
            "--work-dir",
            "--model-dir",
            "--model-variant",
            "--runtime-dir",
            "--gigastt",
            "--chunk-seconds",
            "--asr-timeout-seconds",
            "--retry-window-seconds",
            "--asr-memory-mib",
            "--boundary-window-ms",
            "--encoder-threads",
            "--response",
            "--start",
            "--end",
        ],
        required: &["--work-dir"],
        outputs: &["transcribe2", "transcribe1"],
    },
    Command {
        name: "batch",
        token: "batch",
        aliases: &[],
        positional: Some(("input", true)),
        flags: &[
            "--work-dir",
            "--model-dir",
            "--model-variant",
            "--runtime-dir",
            "--gigastt",
            "--chunk-seconds",
            "--asr-timeout-seconds",
            "--retry-window-seconds",
            "--asr-memory-mib",
            "--boundary-window-ms",
            "--encoder-threads",
            "--jobs",
            "--start",
            "--end",
        ],
        required: &["--work-dir"],
        outputs: &["batch"],
    },
    Command {
        name: "chapters",
        token: "chapters",
        aliases: &[],
        positional: Some(("url", true)),
        flags: &[],
        required: &[],
        outputs: &["chapters"],
    },
    Command {
        name: "setup",
        token: "setup",
        aliases: &[],
        positional: None,
        flags: &[
            "--model-variant",
            "--runtime-dir",
            "--model-dir",
            "--archive",
            "--offline",
        ],
        required: &[],
        outputs: &["setup"],
    },
    Command {
        name: "doctor",
        token: "doctor",
        aliases: &[],
        positional: None,
        flags: &[
            "--work-dir",
            "--model-dir",
            "--model-variant",
            "--runtime-dir",
            "--gigastt",
            "--chunk-seconds",
            "--asr-timeout-seconds",
            "--asr-memory-mib",
            "--boundary-window-ms",
            "--encoder-threads",
        ],
        required: &[],
        outputs: &["doctor"],
    },
    Command {
        name: "status",
        token: "status",
        aliases: &[],
        positional: None,
        flags: &["--work-dir"],
        required: &["--work-dir"],
        outputs: &["job_status"],
    },
    Command {
        name: "cancel",
        token: "cancel",
        aliases: &[],
        positional: None,
        flags: &["--work-dir"],
        required: &["--work-dir"],
        outputs: &["job_cancel"],
    },
    Command {
        name: "resume",
        token: "resume",
        aliases: &[],
        positional: None,
        flags: &["--work-dir", "--response"],
        required: &["--work-dir"],
        outputs: &["transcribe2", "transcribe1"],
    },
    Command {
        name: "export",
        token: "export",
        aliases: &[],
        positional: Some(("input", true)),
        flags: &["--format", "--output", "--force"],
        required: &["--format", "--output"],
        outputs: &["other_success"],
    },
    Command {
        name: "describe",
        token: "describe",
        aliases: &[],
        positional: Some(("command", false)),
        flags: &[],
        required: &[],
        outputs: &["describe"],
    },
    Command {
        name: "help",
        token: "--help",
        aliases: &["-h"],
        positional: None,
        flags: &[],
        required: &[],
        outputs: &["other_success"],
    },
    Command {
        name: "version",
        token: "--version",
        aliases: &["-V"],
        positional: None,
        flags: &[],
        required: &[],
        outputs: &["other_success"],
    },
];
fn diagnostic_flags(name: &str) -> Vec<&'static str> {
    COMMANDS
        .iter()
        .filter(|c| c.name == name)
        .flat_map(|c| std::iter::once(c.token).chain(c.aliases.iter().copied()))
        .collect()
}
fn is_diagnostic(name: &str, arg: &OsStr) -> bool {
    COMMANDS
        .iter()
        .find(|command| command.name == name)
        .is_some_and(|command| {
            arg == command.token || command.aliases.iter().any(|alias| arg == *alias)
        })
}
pub(super) fn is_help(arg: &OsStr) -> bool {
    is_diagnostic("help", arg)
}
pub(super) fn is_version(arg: &OsStr) -> bool {
    is_diagnostic("version", arg)
}
#[derive(Clone, Copy)]
enum Rule {
    ExclusiveRuntime,
    OrderedRange,
    DistinctExportPaths,
}
impl Rule {
    fn describe(self) -> Value {
        match self {
            Self::ExclusiveRuntime => {
                json!({"rule":"mutually_exclusive","parameters":["gigastt","runtime_dir"],"phase":"arguments"})
            }
            Self::OrderedRange => {
                json!({"rule":"end_after_start","parameters":["start","end"],"missing_start_millis":0,"phase":"arguments","max_millis":u64::MAX/16})
            }
            Self::DistinctExportPaths => {
                json!({"rule":"output_differs_from_input","parameters":["input","output"],"existing_path_aliases_checked":true,"phase":"arguments_and_filesystem"})
            }
        }
    }
    fn validate(self, positional: &[OsString], flags: &BTreeMap<String, OsString>) -> Result<()> {
        match self {
            Self::ExclusiveRuntime
                if flags.contains_key("--gigastt") && flags.contains_key("--runtime-dir") =>
            {
                return Err("--gigastt and --runtime-dir cannot be combined".into());
            }
            Self::OrderedRange => {
                let start = flags
                    .get("--start")
                    .map(|v| {
                        crate::time_range::timestamp(v.to_str().ok_or("timestamp must be UTF-8")?)
                    })
                    .transpose()?
                    .unwrap_or(0);
                let end = flags
                    .get("--end")
                    .map(|v| {
                        crate::time_range::timestamp(v.to_str().ok_or("timestamp must be UTF-8")?)
                    })
                    .transpose()?;
                crate::time_range::TimeRange {
                    start_millis: start,
                    end_millis: end,
                }
                .validate()?;
            }
            Self::DistinctExportPaths
                if positional
                    .first()
                    .zip(flags.get("--output"))
                    .is_some_and(|(a, b)| a == b) =>
            {
                return Err("export output must differ from input transcript".into());
            }
            _ => {}
        }
        Ok(())
    }
}
impl Command {
    fn rules(&self) -> Vec<Rule> {
        let mut rules = Vec::new();
        if self.flags.contains(&"--gigastt") {
            rules.push(Rule::ExclusiveRuntime);
        }
        if self.flags.contains(&"--start") {
            rules.push(Rule::OrderedRange);
        }
        if self.name == "export" {
            rules.push(Rule::DistinctExportPaths);
        }
        rules
    }
}
pub(super) fn validate(
    command: &str,
    positional: &[OsString],
    flags: &BTreeMap<String, OsString>,
) -> Result<()> {
    let spec = COMMANDS
        .iter()
        .find(|c| c.token == command)
        .ok_or("unknown command; see describe")?;
    let (min, max) = spec
        .positional
        .map(|(_, required)| (usize::from(required), 1))
        .unwrap_or((0, 0));
    if !(min..=max).contains(&positional.len()) {
        return Err(format!("{command} accepts {min}..{max} positional arguments").into());
    }
    for name in flags.keys() {
        if !spec.flags.contains(&name.as_str()) {
            return Err(format!("option {name} is not valid for {command}").into());
        }
    }
    for required in spec.required {
        if !flags.contains_key(*required) {
            return Err(format!("missing required {required}").into());
        }
    }
    for (name, value) in flags {
        validate_value(name, value)?;
    }
    if spec.positional.is_some_and(|(name, _)| name == "url") {
        pipeline::video_id(positional[0].to_str().ok_or("URL must be UTF-8")?)?;
    }
    for rule in spec.rules() {
        rule.validate(positional, flags)?;
    }
    Ok(())
}
fn field(name: &str) -> String {
    name.trim_start_matches("--").replace('-', "_")
}
fn option_schema(command: &Command, name: &str) -> Result<Value> {
    let p = parameter(name).ok_or("catalog has an unknown parameter")?;
    let mut schema = match p.kind {
        Kind::Path => json!({"type":"string"}),
        Kind::Switch => json!({"type":"boolean","default":false,"x-cli-omit-false":true}),
        Kind::UInt { min, max, default } => {
            let mut v = json!({"type":"integer","minimum":min,"maximum":max});
            if let Some(n) = default {
                v["default"] = json!(n);
            }
            v
        }
        Kind::Timestamp => {
            json!({"type":"string","pattern":"^(?:[0-9]+|[0-9]+:[0-5][0-9]:[0-5][0-9])(?:\\.[0-9]{1,3})?$","x-max-milliseconds":u64::MAX/16})
        }
        Kind::Response => json!({"type":"string","enum":RESPONSES,"default":response_default()}),
        Kind::Model => json!({"type":"string","enum":ModelVariant::ALL.map(ModelVariant::as_str)}),
        Kind::Format => json!({"type":"string","enum":Format::ALL.map(Format::as_str)}),
    };
    schema["x-cli-flag"] = json!(name);
    schema["x-cli-takes-value"] = json!(!p.switch());
    match name {
        "--asr-timeout-seconds" => {
            schema["x-timeout-kind"] = json!("recognition_inactivity");
        }
        "--retry-window-seconds" => {
            schema["x-budget-kind"] = json!("automatic_recovery_wait");
        }
        "--asr-memory-mib" => {
            schema["x-budget-kind"] = json!("observed_recognizer_group_rss");
        }
        _ => {}
    }
    let policy = match name {
        "--work-dir" if command.name == "doctor" => {
            schema["default"] = json!(DOCTOR_ROOT);
            None
        }
        "--runtime-dir" if command.name == "setup" => {
            schema["default"] = json!(crate::runtime::DEFAULT_ROOT);
            None
        }
        "--runtime-dir" => Some(
            json!({"kind":"managed_runtime_if_present","path":crate::runtime::DEFAULT_ROOT,"invalid_runtime":"fail","absent_runtime":"legacy_tool_and_models","explicit_gigastt":"bypasses_discovery"}),
        ),
        "--model-dir" if command.name == "setup" => Some(
            json!({"kind":"runtime_models_subdirectory","omitted":"download_pinned_models","explicit":"local_models_only"}),
        ),
        "--model-dir" => Some(
            json!({"kind":"managed_model_directory_or_home","fallback":format!("$HOME/{LEGACY_MODELS_DIR}")}),
        ),
        "--model-variant" if command.name == "setup" => {
            schema["default"] = json!(ModelVariant::default().as_str());
            None
        }
        "--model-variant" => Some(
            json!({"kind":"prepared_runtime_variant_or_default","fallback":ModelVariant::default().as_str(),"explicit_when_managed":"must_match"}),
        ),
        "--gigastt" => Some(json!({"kind":"managed_executable_or_path","fallback":LEGACY_GIGASTT})),
        "--retry-window-seconds" if command.name == "transcribe" => {
            schema
                .as_object_mut()
                .ok_or("invalid parameter schema")?
                .remove("default");
            Some(json!({"kind":"saved_retry_policy_or_default","fallback":number_default(name)?}))
        }
        "--start" => Some(json!({"kind":"zero_if_end_supplied","otherwise":"whole_recording"})),
        "--end" => {
            Some(json!({"kind":"decoded_end_if_start_supplied","otherwise":"whole_recording"}))
        }
        "--encoder-threads" => Some(json!({"kind":"gigastt_auto"})),
        "--asr-memory-mib" => {
            schema["x-platforms"] = json!(["linux"]);
            Some(json!({"kind":"no_optional_rss_budget"}))
        }
        _ => None,
    };
    if let Some(policy) = policy {
        schema["x-default-policy"] = policy;
    }
    Ok(schema)
}
fn describe_command(command: &Command, contract: &Value) -> Result<Value> {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    if let Some((name, needed)) = command.positional {
        let mut schema = json!({"type":"string","x-cli-position":0});
        if name == "url" {
            schema["format"] = json!("youtube-video-url");
        }
        if name == "input" && command.name == "batch" {
            schema["x-file-input"] = crate::batch::input_description();
        }
        if name == "input" && command.name == "export" {
            schema["x-file-format"] = json!("canonical_transcript_json");
        }
        if name == "command" {
            schema["enum"] = json!(COMMANDS.iter().map(|c| c.name).collect::<Vec<_>>());
        }
        properties.insert(name.into(), schema);
        if needed {
            required.push(name.to_owned());
        }
    }
    for name in command.flags {
        properties.insert(field(name), option_schema(command, name)?);
    }
    required.extend(command.required.iter().map(|name| field(name)));
    let mut constraints: Vec<_> = command.rules().into_iter().map(Rule::describe).collect();
    if command.flags.contains(&"--runtime-dir") && command.name != "setup" {
        constraints.push(json!({"rule":"prepared_runtime_must_be_valid_and_variant_compatible","when":"managed_runtime_selected","phase":"runtime"}));
    }
    if command.name == "resume" {
        constraints.push(json!({"rule":"reuse_saved_job_options","phase":"runtime"}));
    }
    if command.name == "batch" {
        constraints.push(json!({"rule":"immutable_batch_inputs_and_options","phase":"runtime","recognition_workers":1}));
    }
    if command.flags.contains(&"--start") {
        constraints.push(json!({"rule":"resolve_against_full_decoded_audio","phase":"runtime","end_past_source":"clamp","empty_selection":"range_out_of_bounds","download":"full_audio","decode":"full_audio","pcm_storage":"intersecting_fixed_grid_chunks"}));
    }
    let structural: Vec<Value> = command
        .rules()
        .iter()
        .filter_map(|rule| match rule {
            Rule::ExclusiveRuntime => Some(json!({"not":{"required":["gigastt","runtime_dir"]}})),
            _ => None,
        })
        .collect();
    let mut input_schema = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","properties":properties,"required":required,"additionalProperties":false});
    if !structural.is_empty() {
        input_schema["allOf"] = json!(structural);
    }
    let mut variants = Vec::new();
    for definition in command.outputs {
        let version = contract["$defs"][definition]["properties"]["schema_version"]["const"]
            .as_u64()
            .ok_or("output schema lacks envelope version")?;
        let mut variant =
            json!({"schema_ref":format!("#/$defs/{definition}"),"schema_version":version});
        if command.flags.contains(&"--response") {
            let modes: Vec<_> = RESPONSES
                .iter()
                .filter(|mode| u64::from(response_envelope_version(OsStr::new(mode))) == version)
                .copied()
                .collect();
            if modes.is_empty() {
                return Err("response modes disagree with output schema".into());
            }
            variant["when"] = json!({"response":modes});
        }
        variants.push(variant);
    }
    Ok(
        json!({"name":command.name,"argv":[command.token],"aliases":command.aliases,"input_schema":input_schema,"constraints":constraints,"success_variants":variants,"error_schema_ref":"#/$defs/failure"}),
    )
}
fn collect_refs(value: &Value, pending: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            if let Some(name) = fields
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|s| s.strip_prefix("#/$defs/"))
            {
                pending.push(name.into());
            }
            for value in fields.values() {
                collect_refs(value, pending);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_refs(value, pending);
            }
        }
        _ => {}
    }
}
pub(super) fn describe(selector: Option<&OsStr>) -> Result<Value> {
    let selected: Vec<_> = match selector {
        None => COMMANDS.iter().collect(),
        Some(name) => vec![
            COMMANDS
                .iter()
                .find(|c| Some(c.name) == name.to_str())
                .ok_or("unknown description selector; use describe without a selector")?,
        ],
    };
    let contract: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/schemas/contract.schema.json"
    )))?;
    let mut pending: Vec<_> = selected
        .iter()
        .flat_map(|c| c.outputs.iter().map(|name| (*name).to_owned()))
        .collect();
    pending.push("other_success".into());
    pending.push("failure".into());
    pending.push("progress".into());
    pending.push("error_event".into());
    let roots: BTreeSet<_> = pending.iter().cloned().collect();
    let mut definitions = serde_json::Map::new();
    while let Some(name) = pending.pop() {
        if definitions.contains_key(&name) {
            continue;
        }
        let value = contract["$defs"]
            .get(&name)
            .ok_or("missing output schema definition")?
            .clone();
        collect_refs(&value, &mut pending);
        definitions.insert(name, value);
    }
    let output_schema = json!({"$schema":"https://json-schema.org/draft/2020-12/schema","oneOf":roots.iter().map(|name|json!({"$ref":format!("#/$defs/{name}")})).collect::<Vec<_>>(),"$defs":definitions});
    Ok(
        json!({"description_version":1,"application":{"name":"agent-youtube","version":env!("CARGO_PKG_VERSION")},"read_only":true,"runtime_validation":"not_performed",
        "input_schema_semantics":"UTF-8 logical CLI values; not a JSON-input endpoint. Omit false switches; semantic and runtime constraints also apply.",
        "commands":selected.iter().map(|c|describe_command(c,&contract)).collect::<Result<Vec<_>>>()?,"output_schema":output_schema,
        "protocol":{"help_schema_ref":"#/$defs/other_success","stdout":"one_json_document","stderr":"jsonl_events","exit_codes":{"success":0,"failure":1,"interrupted":130},"help_flags":diagnostic_flags("help"),"version_flags":diagnostic_flags("version"),"argument_separator":"--","option_value_syntax":"separate_argv_tokens","empty_argv":"help","duplicate_options":"reject","help_preempts_validation_when_parsed":true,"option_values_are_literal":true,"separator_disables_options":true,"parse_errors_stop_before_later_help":true,"version_standalone":true},
        "recovery":{"authoritative_fields":["error.code","error.retryable","error.details","result.next_action"],"retryable_true":"Respect server waits and next_action before retrying; preserve the work directory.","retryable_false":"Inspect the refusal or arguments; do not loop unchanged.","retryable_null":"Eligibility is unknown; inspect details before retrying.","single_job":"Use status then resume with saved options when permitted.","batch":"Inspect error.details.batch; repeat the same batch only for eligible items and preserved immutable inputs.","ready_setup":"Asset verification does not prove inference or YouTube availability."}}),
    )
}
