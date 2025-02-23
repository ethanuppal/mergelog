// Copyright (C) 2024 Ethan Uppal.
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, version 3 of the License only.
//
// This program is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU General Public License for more
// details.
// You should have received a copy of the GNU General Public License along with
// this program.  If not, see <https://www.gnu.org/licenses/>.

use core::str;
use std::{
    collections::{HashMap, HashSet},
    env,
    error::Error,
    fmt, fs,
    io::{self, Write},
    process::Command,
    str::FromStr,
    time::Duration,
};

use argh::FromArgs;
use camino::{Utf8Path, Utf8PathBuf};
use edit_distance::edit_distance;
use indicatif::{ProgressBar, ProgressStyle};
use miette::{
    miette, Context, Diagnostic, IntoDiagnostic, LabeledSpan, NamedSource,
    Report, Result, SourceOffset,
};
use owo_colors::OwoColorize;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use url::Url;

trait WhateverContextExt<T> {
    fn whatever_context(self, new_parent: Report) -> Result<T>;
}

#[derive(Debug)]
struct DiagnosticWithSource {
    parent: Report,
    cause: Report,
}

impl fmt::Display for DiagnosticWithSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.parent.fmt(f)
    }
}

impl Error for DiagnosticWithSource {}

impl Diagnostic for DiagnosticWithSource {
    fn code<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.parent.code()
    }

    fn severity(&self) -> Option<miette::Severity> {
        self.parent.severity()
    }

    fn help<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.parent.help()
    }

    fn url<'a>(&'a self) -> Option<Box<dyn std::fmt::Display + 'a>> {
        self.parent.url()
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.parent.source_code()
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        self.parent.labels()
    }

    fn related<'a>(
        &'a self,
    ) -> Option<Box<dyn Iterator<Item = &'a dyn Diagnostic> + 'a>> {
        self.parent.related()
    }

    fn diagnostic_source(&self) -> Option<&dyn Diagnostic> {
        Some(self.cause.as_ref())
    }
}

impl<T> WhateverContextExt<T> for std::result::Result<T, Report> {
    fn whatever_context(self, new_parent: Report) -> Result<T> {
        self.map_err(|cause| {
            Report::new(DiagnosticWithSource {
                parent: new_parent,
                cause,
            })
        })
    }
}

impl<T> WhateverContextExt<T> for Option<T> {
    fn whatever_context(self, new_parent: Report) -> Result<T> {
        self.ok_or(new_parent)
    }
}

#[derive(Clone, Copy)]
enum RepositoryHost {
    GitHub,
    GitLab,
    Infer,
}

impl FromStr for RepositoryHost {
    type Err = Report;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "github" | "gh" => Ok(Self::GitHub),
            "gitlab" | "gl" => Ok(Self::GitLab),
            other => Err(miette!("Failed to parse '{other}' as a repository host. Options include 'github'/'gh for GitHub and 'gitlab'/'gl' for GitLab"))
        }
    }
}

/// Merges changelog files into a single changelog
#[derive(FromArgs)]
struct Opts {
    /// link to the repository to resolve merge/pull requests at; omit to infer
    /// from the current repo
    #[argh(option, long = "repo")]
    repo_url: Option<Url>,

    /// the repository host; omit to infer from the repo URL
    #[argh(option, default = "RepositoryHost::Infer")]
    host: RepositoryHost,

    /// changelog sections in order
    #[argh(option, short = 's')]
    section: Vec<String>,

    /// path to optional config file
    #[argh(option)]
    config: Option<Utf8PathBuf>,

    /// directory containing changelogs and a mergelog.toml
    #[argh(positional)]
    changelog_directory: Utf8PathBuf,
}

fn default_config_format() -> String {
    "{item} ({link_name})".into()
}

#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    sections: Vec<String>,
    #[serde(default = "default_config_format")]
    format: String,
    #[serde(default, rename = "short-links")]
    short_links: bool,
}

struct PullRequest {
    id: u64,
    link: String,
    title: String,
}

impl PullRequest {
    fn try_from_gitlab(value: &JsonValue) -> Result<Self> {
        let id = value
            .get("iid")
            .and_then(|value| value.as_u64())
            .wrap_err("Missing 'iid' field on merge request")?;
        let name = value
            .get("title")
            .and_then(|value| value.as_str())
            .wrap_err("Missing 'name' field on merge request")?;
        Ok(Self {
            id,
            link: format!("!{}", id),
            title: name.to_string(),
        })
    }
}

/// # Safety
///
/// `substring` must start after `source`, although this function only makes
/// sense if the start of `substring` is within the range of `source`.
unsafe fn start_in(source: &str, substring: &str) -> usize {
    substring.as_ptr().offset_from(source.as_ptr()) as usize
}

fn infer_host(repo_url: &Url) -> Result<RepositoryHost> {
    if let Some(domain) = repo_url.domain() {
        match domain {
            "github.com" => Ok(RepositoryHost::GitHub),
            "gitlab.com" => Ok(RepositoryHost::GitLab),
            _ => {
                let start = unsafe { start_in(domain, repo_url.as_str()) };
                Err(miette!(
                    code = "infer_host::unknown_domain",
                    labels = vec![LabeledSpan::new_with_span(None, (start, domain.len()))],
                    help = "Please use a known repository host like github.com or gitlab.com.",
                    "Unknown host domain"
                )
                .with_source_code(NamedSource::new("url",repo_url.to_string())))
            }
        }
    } else {
        Err(miette!(
            code = "infer_host::missing_domain",
            "Provided URL missing domain"
        )
        .with_source_code(NamedSource::new("url", repo_url.to_string())))
    }
}

fn parse_owner_and_name(
    url: Url,
    host: RepositoryHost,
) -> Result<(String, String)> {
    match host {
        RepositoryHost::GitHub => todo!(),
        RepositoryHost::GitLab => {
            let components = url
                .path_segments()
                .wrap_err("Repository URL missing path segments")?
                .collect::<Vec<_>>();
            if components.len() < 2
                || (components.len() == 2
                    && (components[0].is_empty() || components[1].is_empty()))
            {
                let start = if components.is_empty() {
                    0
                } else {
                    unsafe { start_in(url.as_str(), components[0]) }
                };
                let length = url.as_str().len() - start;
                return Err(miette!(
                    code = "parse_owner_and_name::incorrect_format",
                    labels = vec![LabeledSpan::at(
                        (start, length),
                        "less than two path segments"
                    )],
                    help = "The URL should be of the form: https://gitlab.com/{owner}/{name}",
                    "URL does not point to a repository"
                )
                .with_source_code(NamedSource::new("url", url.to_string())));
            }
            Ok((components[0].to_string(), components[1].to_string()))
        }
        RepositoryHost::Infer => unreachable!(),
    }
}

fn fetch_merge_requests(
    owner: &str,
    name: &str,
    host: RepositoryHost,
) -> Result<Vec<PullRequest>> {
    match host {
        RepositoryHost::GitHub => todo!(),
        RepositoryHost::GitLab => {
            let request = format!("https://gitlab.com/api/v4/projects/{}%2F{}/merge_requests?state=merged&view=simple&per_page=100", owner, name);
            let response = reqwest::blocking::get(&request)
                .into_diagnostic()
                .whatever_context(miette!(
                    code = "fetch_merge_requests::api_error",
                    "Failed to obtain merge requests from {}/{}",
                    owner,
                    name
                ))?
                .text()
                .into_diagnostic()
                .whatever_context(miette!(
                    "Failed to extract GitLab API response text"
                ))?;
            let response_json: JsonValue = serde_json::from_str(&response)
                .map_err(|cause| {
                    miette!(
                        code = "fetch_merge_requests::serde_json_error",
                        labels = vec![LabeledSpan::at(
                            SourceOffset::from_location(
                                &response,
                                cause.line(),
                                cause.column()
                            ),
                            cause.to_string()
                        )],
                        "Failed to parse GitLab API response text"
                    )
                    .with_source_code(
                        NamedSource::new(request.as_str(), response.clone())
                            .with_language("json"),
                    )
                })?;
            let merge_requests = response_json.as_array().whatever_context(
                miette!(
                    code = "fetch_merge_requests::malformed_json",
                    labels = vec![LabeledSpan::at(
                        (0, 0),
                        "Expected array of merge request details"
                    )],
                    "Failed to parse GitLab API response text"
                )
                .with_source_code(
                    NamedSource::new(request, response).with_language("json"),
                ),
            )?;
            merge_requests
                .iter()
                .map(PullRequest::try_from_gitlab)
                .collect::<Result<Vec<_>>>()
        }
        RepositoryHost::Infer => unreachable!(),
    }
}

fn prompt<'a>(
    prompt: impl Fn(),
    validate: impl Fn(&str) -> bool,
    exit: impl Fn(&str),
    default: impl Into<Option<&'a str>>,
) -> Result<String> {
    let default = default.into().map(Into::into);
    let mut buffer = String::new();
    loop {
        prompt();
        io::stdout()
            .flush()
            .into_diagnostic()
            .wrap_err("Failed to flush standard output")?;
        io::stderr()
            .flush()
            .into_diagnostic()
            .wrap_err("Failed to flush standard input")?;
        io::stdin()
            .read_line(&mut buffer)
            .into_diagnostic()
            .wrap_err("Failed to read user input")?;
        let buffer = buffer.trim();
        if buffer.is_empty() {
            if let Some(default) = default {
                exit(default);
                return Ok(default.to_string());
            }
        };
        if validate(buffer) {
            exit(buffer);
            return Ok(buffer.to_string());
        }
    }
}

fn guess_pull_request<'a>(
    name: &str,
    pull_requests: &'a [PullRequest],
) -> Option<Vec<&'a PullRequest>> {
    let mut costs = pull_requests
        .iter()
        .enumerate()
        .map(|(i, pr)| {
            let words = pr.title.split_ascii_whitespace().collect::<Vec<_>>();
            let distance = words
                .iter()
                .map(|word| {
                    if name.to_lowercase().contains(&word.to_lowercase())
                        && word.len() > 1
                    {
                        pr.title.len() * 10
                    } else {
                        edit_distance(&pr.title, name)
                    }
                })
                .sum::<usize>() as f64;
            let normalizer = if words.is_empty() { 1 } else { pr.title.len() };
            (i, distance / (normalizer as f64))
        })
        .collect::<Vec<_>>();
    if costs.is_empty() {
        return None;
    }
    costs.sort_by(|lhs, rhs| {
        lhs.1
            .partial_cmp(&rhs.1)
            .expect("we should not have created NaNs")
            .reverse()
    });
    Some(
        costs
            .into_iter()
            .take(5)
            .map(|(index, _)| &pull_requests[index])
            .collect(),
    )
}

#[derive(Clone)]
struct Link {
    shorthand: String,
    full: String,
}

fn make_pull_request_link(
    id: String,
    link: String,
    host: RepositoryHost,
    repo_owner: &str,
    repo_name: &str,
) -> Link {
    let full_link = match host {
        RepositoryHost::GitHub => todo!(),
        RepositoryHost::GitLab => {
            format!(
                "https://gitlab.com/{repo_owner}/{repo_name}/-/merge_requests/{id}"
            )
        }
        RepositoryHost::Infer => unreachable!(),
    };
    Link {
        shorthand: link,
        full: full_link,
    }
}

/// Determines the link for the changelog entry. If the entry name is not a
/// number, it tries to guess from the pull requests and asks the user.
fn resolve_changelog_pr_interactive(
    name: &str,
    contents: &str,
    pull_requests: &[PullRequest],
    repo_owner: &str,
    repo_name: &str,
    host: RepositoryHost,
) -> Result<Link> {
    if let Ok(id) = name.parse::<u64>() {
        let link = if let Some(link) = pull_requests
            .iter()
            .find(|pr| pr.id == id)
            .map(|pr| pr.link.clone())
        {
            eprintln!(
                "✓ {}",
                format!("Processing changelog for {}", link).green()
            );
            link
        } else {
            prompt(
                || {
                    eprint!("TODO: fix gitlab api requests to do pagination.\nfor now just tell me if it's ok (y/n):");
                },
                |value| ["y", "n"].contains(&value),
                |value| {
                    eprintln!(
                        "✓ {}",
                        format!("Processing changelog for {}", value).green()
                    )
                },
                "y",
            )?
        };
        Ok(make_pull_request_link(
            id.to_string(),
            link,
            host,
            repo_owner,
            repo_name,
        ))
    } else {
        eprintln!(
            "╭─ {}:",
            format!("Cannot automatically determine pull request for changelog '{}.md', if it even has one", name).red(),
        );
        eprintln!("│");
        for line in contents.lines() {
            eprintln!("│ {}", line.fg_rgb::<128, 128, 128>());
        }
        eprintln!("│");
        if let Some(guessed_prs) = guess_pull_request(name, pull_requests) {
            eprintln!("├─ {}: Is it one of:", "help".cyan());
            for guessed_pr in guessed_prs {
                eprintln!(
                    "│          {}: {}",
                    guessed_pr.link, guessed_pr.title
                );
            }
            eprintln!("│");
        }
        let full_link = prompt(
            || {
                eprint!("╰─ Please enter the desired link (can also be a link like !30 in GitLab): ")
            },
            |value| !value.is_empty(),
            |value| {
                eprintln!(
                    "✓ {}",
                    format!("Processing changelog for {}", value).green()
                )
            },
            None,
        )?;
        if let Some(id) = match host {
            RepositoryHost::GitHub => todo!(),
            RepositoryHost::GitLab => full_link.strip_prefix("!"),
            RepositoryHost::Infer => unreachable!(),
        } {
            Ok(make_pull_request_link(
                id.to_string(),
                full_link,
                host,
                repo_owner,
                repo_name,
            ))
        } else {
            let shorthand = prompt(
                || {
                    eprint!("   Please provide the markdown shorthand name for the link: ")
                },
                |value| !value.is_empty(),
                |_| {},
                None,
            )?;
            Ok(Link {
                shorthand,
                full: full_link,
            })
        }
    }
}

fn load_config(path: Utf8PathBuf) -> Result<Config> {
    let contents = fs::read_to_string(&path)
        .into_diagnostic()
        .wrap_err(format!("Failed to read config file from {}", path))?;
    toml::from_str(&contents).map_err(|cause| {
        let labels = cause
            .span()
            .into_iter()
            .map(|span| LabeledSpan::at(span, cause.to_string()))
            .collect::<Vec<_>>();
        miette!(
            code = "load_config::toml_error",
            labels = labels,
            "Failed to parse config file"
        )
        .with_source_code(
            NamedSource::new(path, contents).with_language("toml"),
        )
    })
}

fn main() -> Result<()> {
    let mut opts = argh::from_env::<Opts>();

    let (format, short_links) = if let Some(config_path) =
        opts.config.or_else(|| {
            if Utf8Path::new("mergelog.toml").is_file() {
                Some(Utf8Path::new("mergelog.toml").to_path_buf())
            } else {
                None
            }
        }) {
        let config = load_config(config_path.clone())?;
        eprintln!(
            "✓ {}",
            format!("Loaded config from {}", config_path).green()
        );
        if opts.section.is_empty() {
            opts.section = config.sections;
        }
        (config.format, config.short_links)
    } else {
        (default_config_format(), false)
    };

    // TODO: bad if there are escaped characters
    let command_as_string = env::args().collect::<Vec<_>>().join(" ");

    if !opts.changelog_directory.is_dir() {
        let dir_string = opts.changelog_directory.as_str();
        let start = command_as_string
            .find(dir_string)
            .expect("TODO: handle escapes. you get no pretty error but TLDR the changelog directory you specified does not exist :(");
        return Err(miette!(
            code = "main::missing_changelogs",
            labels = vec![LabeledSpan::at(
                (start, dir_string.len()),
                "Directory specified here"
            )],
            "Changelog directory specified either does not exist or is not a directory"
        )
        .with_source_code(command_as_string));
    }

    if opts.section.is_empty() {
        return Err(miette!(
            code = "main::missing_sections",
            labels = vec![LabeledSpan::at(0..command_as_string.len(), "Missing section option(s)")],
            help = "Provide a changelog section by passing the option `-s`/--section` multiple times, e.g., `-s Added`.\n\nThese sections correspond to markdown headings in the changelog files, and the order in which you pass the sections is the order in which they will be generated in the changelog.", 
            "No changelog sections provided"
        ).with_source_code(command_as_string));
    }

    let repo_url = if let Some(repo_url) = opts.repo_url {
        repo_url
    } else {
        let git_output = Command::new("git")
            .args(["config", "--get", "remote.origin.url"])
            .output()
            .into_diagnostic()
            .wrap_err("Failed to determine origin URL in current repository")?;
        let origin_string = String::from_utf8(git_output.stdout)
            .into_diagnostic()
            .wrap_err("Failed to decode origin URL as UTF-8")?;
        Url::parse(&origin_string).map_err(|inner| {
            let help = if origin_string.is_empty() {
                "Add a valid remote origin URL with `git remote add origin <url>`. You can also specify the URL manually by passing `--repo`"
            } else {
                "Remove the current remote origin with `git remote remove origin` and readd a correct one. You can also specify the URL manually by passing `--repo`"
            };
            miette!(
                code = "main::parse_url",
                labels = vec![LabeledSpan::at(
                    (0, origin_string.len()),
                    inner.to_string()
                )],
                help = help,
                "Failed to parse {}origin URL",
                if origin_string.is_empty() { "empty " } else { "" }
            )
            .with_source_code(NamedSource::new("url", origin_string))
        })?
    };
    let host = match opts.host {
        RepositoryHost::Infer => infer_host(&repo_url)?,
        specified => specified,
    };

    let (repo_owner, repo_name) = parse_owner_and_name(repo_url, host)?;

    let spinner = ProgressBar::new_spinner()
        .with_message("Fetching information from remote repository")
        .with_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈✓"),
        );
    spinner.enable_steady_tick(Duration::from_millis(100));
    let pull_requests = fetch_merge_requests(&repo_owner, &repo_name, host)?;
    spinner.finish_with_message(
        "Fetched information from remote repository"
            .green()
            .to_string(),
    );

    let mut sections = HashMap::<String, (u8, Vec<(String, Link)>)>::new();
    let mut current_section = None;

    let arena = comrak::Arena::new();
    if let Ok(read_dir) = opts.changelog_directory.read_dir_utf8() {
        for entry in read_dir.flatten() {
            if entry.path().is_file()
                && entry
                    .path()
                    .extension()
                    .map(|extension| extension == "md")
                    .unwrap_or(false)
            {
                let Some(file_stem) = entry.path().file_stem() else {
                    continue;
                };

                let changelog_contents = fs::read_to_string(entry.path())
                    .into_diagnostic()
                    .whatever_context(miette!(
                        code = "main::io_error",
                        "Failed to read changelog at {}",
                        entry.path()
                    ))?;

                let link = resolve_changelog_pr_interactive(
                    file_stem,
                    &changelog_contents,
                    &pull_requests,
                    &repo_owner,
                    &repo_name,
                    host,
                )?;

                for node in comrak::parse_document(
                    &arena,
                    &changelog_contents,
                    &comrak::Options::default(),
                )
                .descendants()
                {
                    match node.data.borrow().value {
                        comrak::nodes::NodeValue::Heading(heading) => {
                            let mut heading_string = String::new();
                            for descendant in node.children() {
                                match descendant.data.borrow().value {
                                    comrak::nodes::NodeValue::Text(
                                        ref text,
                                    ) => heading_string.push_str(text),
                                    _ => todo!(),
                                }
                            }
                            current_section = Some((
                                heading_string.trim().to_string(),
                                heading.level,
                            ));
                        }
                        comrak::nodes::NodeValue::Item(_) => {
                            let mut result = Vec::new();
                            comrak::format_commonmark(
                                node,
                                &comrak::Options::default(),
                                &mut result,
                            )
                            .into_diagnostic()
                            .wrap_err("Failed to format document")?;
                            let result = String::from_utf8(result)
                                .into_diagnostic()
                                .wrap_err(
                                    "Markdown list item was not valid UTF-8",
                                )?;
                            if let Some(current_section) =
                                current_section.as_ref()
                            {
                                sections
                                    .entry(current_section.0.clone())
                                    .or_insert((current_section.1, vec![]))
                                    .1
                                    .push((result, link.clone()));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let mut short_links_set = HashSet::new();
    for (i, section) in opts.section.into_iter().enumerate() {
        if i > 0 {
            println!();
        }
        if let Some((level, contents)) = sections.get_mut(&section) {
            contents.sort_by(|lhs, rhs| lhs.1.shorthand.cmp(&rhs.1.shorthand));
            println!("{} {}", "#".repeat(*level as usize), section);
            for (content, link) in contents {
                let item = content.trim();
                let item = item.strip_prefix("-").unwrap_or(item).trim();
                println!(
                    "- {}",
                    format
                        .replace("{link_short}", &link.shorthand)
                        .replace("{link}", &link.full)
                        .replace("{item}", item)
                );
                if short_links {
                    short_links_set
                        .insert((link.shorthand.clone(), link.full.clone()));
                }
            }
        }
    }
    if !short_links_set.is_empty() {
        println!();
        let mut short_links_list =
            short_links_set.into_iter().collect::<Vec<_>>();
        short_links_list.sort();
        for (link, full_link) in short_links_list {
            println!("[{link}]: {full_link}");
        }
    }

    Ok(())
}
