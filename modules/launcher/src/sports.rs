//! Sports scores: `nba`, `nfl scores`, `scores premier league`, `nba lakers`, `ucl`.
//!
//! Scoreboards come from ESPN's public scoreboard endpoint. It is unofficial and keyless; it
//! can change without notice, so failures are reported plainly. The league name is the only
//! thing sent, and only after you type a league keyword.

use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    web::{self, Lookup, WebCache},
};
use std::time::Duration;

const API: &str = "https://site.api.espn.com/apis/site/v2/sports/";
/// Live games change quickly; keep the cache short.
const TTL: Duration = Duration::from_secs(45);
const MAX_GAMES: usize = 6;

/// A league ESPN publishes a scoreboard for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct League {
    /// Short key used in ids and cache keys.
    key: &'static str,
    label: &'static str,
    /// `<sport>/<league>` in the API.
    api_path: &'static str,
    /// Path segment on espn.com for a game page.
    web_path: &'static str,
    aliases: &'static [&'static str],
}

const LEAGUES: &[League] = &[
    League { key: "nba", label: "NBA", api_path: "basketball/nba", web_path: "nba", aliases: &["nba"] },
    League { key: "wnba", label: "WNBA", api_path: "basketball/wnba", web_path: "wnba", aliases: &["wnba"] },
    League { key: "nfl", label: "NFL", api_path: "football/nfl", web_path: "nfl", aliases: &["nfl"] },
    League { key: "mlb", label: "MLB", api_path: "baseball/mlb", web_path: "mlb", aliases: &["mlb"] },
    League { key: "nhl", label: "NHL", api_path: "hockey/nhl", web_path: "nhl", aliases: &["nhl"] },
    League { key: "epl", label: "Premier League", api_path: "soccer/eng.1", web_path: "soccer", aliases: &["premier league", "epl", "english premier league"] },
    League { key: "laliga", label: "La Liga", api_path: "soccer/esp.1", web_path: "soccer", aliases: &["la liga", "laliga"] },
    League { key: "bundesliga", label: "Bundesliga", api_path: "soccer/ger.1", web_path: "soccer", aliases: &["bundesliga"] },
    League { key: "seriea", label: "Serie A", api_path: "soccer/ita.1", web_path: "soccer", aliases: &["serie a", "seriea"] },
    League { key: "ligue1", label: "Ligue 1", api_path: "soccer/fra.1", web_path: "soccer", aliases: &["ligue 1", "ligue1"] },
    League { key: "mls", label: "MLS", api_path: "soccer/usa.1", web_path: "soccer", aliases: &["mls"] },
    League { key: "ucl", label: "Champions League", api_path: "soccer/uefa.champions", web_path: "soccer", aliases: &["champions league", "ucl"] },
];

fn league_by_key(key: &str) -> Option<&'static League> {
    LEAGUES.iter().find(|league| league.key == key)
}

/// Words that may surround a league name.
const NOISE: [&str; 9] = ["scores", "score", "results", "live", "games", "game", "today", "matches", "fixtures"];

/// `nba lakers` -> (NBA, Some("lakers")); `scores premier league` -> (EPL, None).
fn parse_query(query: &str) -> Option<(&'static League, Option<String>)> {
    let lower = query.trim().to_lowercase();
    let words: Vec<&str> = lower
        .split_whitespace()
        .filter(|word| !NOISE.contains(word))
        .collect();

    if words.is_empty() {
        return None;
    }

    let rest = words.join(" ");

    // Longest alias wins (`premier league` before a hypothetical `league`).
    let (league, alias) = LEAGUES
        .iter()
        .flat_map(|league| league.aliases.iter().map(move |alias| (league, *alias)))
        .filter(|(_, alias)| {
            rest == *alias || rest.starts_with(&format!("{alias} ")) || rest.ends_with(&format!(" {alias}"))
        })
        .max_by_key(|(_, alias)| alias.len())?;

    let team = rest
        .replacen(alias, "", 1)
        .trim()
        .to_owned();

    let team_ok = team.chars().count() <= 30
        && team.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '\'' | '-'));

    team_ok.then(|| (league, (!team.is_empty()).then_some(team)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Live,
    Upcoming,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Side {
    name: String,
    short: String,
    abbreviation: String,
    score: Option<String>,
    winner: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Game {
    id: String,
    phase: Phase,
    /// ESPN's own short status: `Final`, `Q3 4:12`, `10/11 - 7:30 PM EDT`.
    status: String,
    home: Side,
    away: Side,
}

fn parse_side(competitor: &serde_json::Value) -> Option<Side> {
    let team = competitor.get("team")?;
    let text = |key: &str| team.get(key).and_then(|v| v.as_str()).map(str::to_owned);

    let name = text("displayName").or_else(|| text("name"))?;

    Some(Side {
        short: text("shortDisplayName").unwrap_or_else(|| name.clone()),
        abbreviation: text("abbreviation").unwrap_or_default(),
        name,
        score: competitor
            .get("score")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        winner: competitor
            .get("winner")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

fn parse_games(value: &serde_json::Value) -> Option<Vec<Game>> {
    let events = value.get("events")?.as_array()?;

    let mut games: Vec<Game> = events
        .iter()
        .filter_map(|event| {
            let id = event.get("id")?.as_str()?.to_owned();
            let status = event.get("status")?.get("type")?;
            let state = status.get("state")?.as_str()?;

            let phase = match state {
                "in" => Phase::Live,
                "post" => Phase::Finished,
                _ => Phase::Upcoming,
            };

            let short = status
                .get("shortDetail")
                .or_else(|| status.get("description"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();

            let competitors = event.get("competitions")?.as_array()?.first()?.get("competitors")?.as_array()?;
            let side = |role: &str| {
                competitors
                    .iter()
                    .find(|competitor| competitor.get("homeAway").and_then(|v| v.as_str()) == Some(role))
                    .and_then(parse_side)
            };

            Some(Game {
                id,
                phase,
                status: short,
                home: side("home")?,
                away: side("away")?,
            })
        })
        .collect();

    // Live first, then what starts next, then results; the feed's order within each.
    games.sort_by_key(|game| game.phase);

    Some(games)
}

fn matches_team(game: &Game, filter: &str) -> bool {
    [&game.home, &game.away].iter().any(|side| {
        side.name.to_lowercase().contains(filter)
            || side.short.to_lowercase().contains(filter)
            || side.abbreviation.eq_ignore_ascii_case(filter)
    })
}

fn result_for(league: &League, game: &Game, rank: usize) -> SearchResult {
    let title = match (game.phase, &game.away.score, &game.home.score) {
        (Phase::Upcoming, _, _) | (_, None, _) | (_, _, None) => {
            format!("{} at {}", game.away.short, game.home.short)
        }
        (_, Some(away), Some(home)) => format!(
            "{} {away} \u{2013} {home} {}",
            game.away.short, game.home.short
        ),
    };

    let status = match game.phase {
        Phase::Live => format!("\u{25cf} Live \u{b7} {}", game.status),
        _ => game.status.clone(),
    };

    // `away|home|away score|home score|winner` lets the UI draw a scoreboard card.
    let detail = format!(
        "{}|{}|{}|{}|{}",
        game.away.name,
        game.home.name,
        game.away.score.as_deref().unwrap_or(""),
        game.home.score.as_deref().unwrap_or(""),
        match (game.away.winner, game.home.winner) {
            (true, _) => "away",
            (_, true) => "home",
            _ => "",
        }
    );

    SearchResult::new(
        format!("sports:{}:{}", league.key, game.id),
        title,
        ResultKind::Sports,
        1850 - rank as u32,
    )
    .with_subtitle(format!("{status} \u{b7} {}", league.label))
    .with_detail(detail)
}

fn fetch(league: &League) -> Result<Vec<Game>, String> {
    let value = web::get_json(&format!("{API}{}/scoreboard", league.api_path))?;

    parse_games(&value).ok_or_else(|| "the scores service sent an unexpected answer".to_owned())
}

pub struct SportsProvider {
    cache: WebCache<Vec<Game>>,
}

impl SportsProvider {
    pub fn new() -> Self {
        Self {
            cache: WebCache::new(),
        }
    }
}

impl Default for SportsProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for SportsProvider {
    fn id(&self) -> &'static str {
        "sports"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let Some((league, team)) = parse_query(query) else {
            return Vec::new();
        };

        let what = format!("{} scores", league.label);
        let target = *league;

        match self.cache.get(league.key, TTL, move || fetch(&target)) {
            Lookup::Loading => vec![web::loading_result(league.key, &what)],
            Lookup::Failed(reason) => vec![web::failure_result(league.key, &what, &reason)],
            Lookup::Ready(games) => {
                let shown: Vec<SearchResult> = games
                    .iter()
                    .filter(|game| team.as_deref().is_none_or(|team| matches_team(game, team)))
                    .take(MAX_GAMES)
                    .enumerate()
                    .map(|(rank, game)| result_for(league, game, rank))
                    .collect();

                if shown.is_empty() {
                    let message = match &team {
                        Some(team) => format!("No {} game for \u{201c}{team}\u{201d} today", league.label),
                        None => format!("No {} games today", league.label),
                    };

                    vec![
                        SearchResult::new(
                            format!("notice:{}", league.key),
                            message,
                            ResultKind::Notice,
                            1850,
                        )
                        .with_subtitle("The scoreboard shows the current day's games"),
                    ]
                } else {
                    shown
                }
            }
        }
    }

    /// Enter opens the game page on espn.com.
    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let rest = id.strip_prefix("sports:")?;
        let (key, game) = rest.split_once(':')?;

        let Some(league) = league_by_key(key) else {
            return Some(Err("unknown league".to_owned()));
        };

        if game.is_empty() || !game.chars().all(|c| c.is_ascii_digit()) {
            return Some(Err("not a game id".to_owned()));
        }

        let page = if league.web_path == "soccer" { "match" } else { "game" };

        Some(Ok(LaunchSpec {
            title: format!("{} game", league.label),
            program: "xdg-open".to_owned(),
            args: vec![format!(
                "https://www.espn.com/{}/{page}/_/gameId/{game}",
                league.web_path
            )],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, state: &str, detail: &str, away: (&str, &str), home: (&str, &str)) -> String {
        format!(
            r#"{{"id":"{id}","status":{{"type":{{"state":"{state}","shortDetail":"{detail}"}}}},
            "competitions":[{{"competitors":[
              {{"homeAway":"home","score":"{}","winner":false,"team":{{"displayName":"{} Town","shortDisplayName":"{}","abbreviation":"HOM"}}}},
              {{"homeAway":"away","score":"{}","winner":true,"team":{{"displayName":"{} City","shortDisplayName":"{}","abbreviation":"AWY"}}}}
            ]}}]}}"#,
            home.1, home.0, home.0, away.1, away.0, away.0
        )
    }

    fn sample() -> serde_json::Value {
        let events = [
            event("1", "post", "Final", ("Lakers", "99"), ("Celtics", "102")),
            event("2", "in", "Q3 4:12", ("Bulls", "70"), ("Heat", "65")),
            event("3", "pre", "10/11 - 7:30 PM EDT", ("Nets", "0"), ("Knicks", "0")),
        ];

        serde_json::from_str(&format!(r#"{{"events":[{}]}}"#, events.join(","))).unwrap()
    }

    #[test]
    fn recognises_leagues_and_teams() {
        assert_eq!(parse_query("nba").unwrap().0.key, "nba");
        assert_eq!(parse_query("NBA scores").unwrap().0.key, "nba");
        assert_eq!(parse_query("scores premier league").unwrap().0.key, "epl");
        assert_eq!(parse_query("la liga").unwrap().0.key, "laliga");

        let (league, team) = parse_query("nba lakers").unwrap();

        assert_eq!((league.key, team.as_deref()), ("nba", Some("lakers")));

        let (league, team) = parse_query("arsenal premier league").unwrap();

        assert_eq!((league.key, team.as_deref()), ("epl", Some("arsenal")));
    }

    #[test]
    fn ignores_everything_else() {
        assert!(parse_query("firefox").is_none());
        assert!(parse_query("scores").is_none());
        assert!(parse_query("").is_none());
        assert!(parse_query("nbafoo").is_none());
        assert!(parse_query("nba $(x)").is_none());
    }

    #[test]
    fn orders_live_then_upcoming_then_final() {
        let games = parse_games(&sample()).unwrap();
        let order: Vec<&str> = games.iter().map(|game| game.id.as_str()).collect();

        assert_eq!(order, ["2", "3", "1"]);
    }

    #[test]
    fn titles_show_scores_only_once_a_game_has_started() {
        let games = parse_games(&sample()).unwrap();
        let nba = league_by_key("nba").unwrap();

        let live = result_for(nba, &games[0], 0);
        let upcoming = result_for(nba, &games[1], 1);
        let done = result_for(nba, &games[2], 2);

        assert_eq!(live.title, "Bulls 70 \u{2013} 65 Heat");
        assert!(live.subtitle.as_deref().unwrap().starts_with("\u{25cf} Live \u{b7} Q3 4:12"));
        assert_eq!(upcoming.title, "Nets at Knicks");
        assert_eq!(done.title, "Lakers 99 \u{2013} 102 Celtics");
        assert!(done.detail.unwrap().ends_with("|away"));
        assert!(live.score > upcoming.score && upcoming.score > done.score);
    }

    #[test]
    fn filters_by_team() {
        let games = parse_games(&sample()).unwrap();

        assert!(matches_team(&games[0], "bulls"));
        assert!(matches_team(&games[0], "heat town"));
        assert!(matches_team(&games[0], "awy"));
        assert!(!matches_team(&games[0], "lakers"));
    }

    #[test]
    fn provider_serves_cache_and_reports_empty_days() {
        let provider = SportsProvider::new();

        provider.cache.insert("nba", parse_games(&sample()).unwrap());

        assert_eq!(provider.search("nba").len(), 3);
        assert_eq!(provider.search("nba bulls").len(), 1);

        let none = provider.search("nba zebras");

        assert_eq!(none.len(), 1);
        assert_eq!(none[0].kind, ResultKind::Notice);
    }

    #[test]
    fn resolves_only_known_leagues_and_numeric_ids() {
        let provider = SportsProvider::new();

        assert_eq!(
            provider.resolve("sports:nba:401").unwrap().unwrap().args,
            ["https://www.espn.com/nba/game/_/gameId/401"]
        );
        assert_eq!(
            provider.resolve("sports:epl:7").unwrap().unwrap().args,
            ["https://www.espn.com/soccer/match/_/gameId/7"]
        );
        assert!(provider.resolve("sports:nba:../x").unwrap().is_err());
        assert!(provider.resolve("sports:xxx:1").unwrap().is_err());
    }
}
