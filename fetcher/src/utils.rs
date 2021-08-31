use crate::{Fetcher, ReturnAction};
use reqwest;
use std::time::Duration;

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/92.0.4515.131 Safari/537.36";
const FIELDS: [&str; 3] = [
    "fields=videoId,title,author,lengthSeconds",
    "fields=title,playlistId,author,videoCount",
    "fields=author,authorId,videoCount",
];
pub const ITEM_PER_PAGE: usize = 10;
const REGION: &str = "region=NP";
const FILTER_TYPE: [&str; 3] = ["music", "playlist", "channel"];

impl crate::ExtendDuration for Duration {
    fn to_string(self) -> String {
        let seconds: u64 = self.as_secs();
        let mut res = format!(
            "{minutes}:{seconds}",
            minutes = seconds / 60,
            seconds = seconds % 60
        );
        res.shrink_to_fit();
        res
    }

    // This function assumes that the string is alwayd formatted in "min:secs"
    fn from_string(inp: &str) -> Duration {
        let splitted = inp.split_once(':').unwrap();
        let total_secs: u64 = (60 * splitted.0.trim().parse::<u64>().unwrap_or_default())
            + splitted.1.trim().parse::<u64>().unwrap_or_default();
        Duration::from_secs(total_secs)
    }
}

impl Fetcher {
    pub fn new() -> Self {
        super::Fetcher {
            trending_now: None,
            playlist_content: (String::new(), Vec::new()),
            search_res: (
                String::new(),
                super::SearchRes {
                    music: Vec::new(),
                    playlist: Vec::new(),
                    artist: Vec::new(),
                    last_fetched: -1,
                },
            ),
            servers: [
                "https://invidious.snopyta.org/api/v1",
                "https://vid.puffyan.us/api/v1",
                "https://ytprivate.com/api/v1",
                "https://ytb.trom.tf/api/v1",
                "https://invidious.namazso.eu/api/v1",
                "https://invidious.hub.ne.kr/api/v1",
            ],
            client: reqwest::ClientBuilder::default()
                .user_agent(USER_AGENT)
                .gzip(true)
                .build()
                .unwrap(),
            active_server_index: 0,
        }
    }
    pub fn change_server(&mut self) {
        self.active_server_index = (self.active_server_index + 1) % self.servers.len();
    }
}

macro_rules! search {
    ("music", $fetcher: expr, $query: expr, $page: expr) => {
        search!(
            "@internal-core",
            $fetcher,
            $query,
            $page,
            $fetcher.search_res.1.music,
            0,
            super::MusicUnit
        )
    };
    ("playlist", $fetcher: expr, $query: expr, $page: expr) => {
        search!(
            "@internal-core",
            $fetcher,
            $query,
            $page,
            $fetcher.search_res.1.playlist,
            1,
            super::PlaylistUnit
        )
    };
    ("artist", $fetcher: expr, $query: expr, $page: expr) => {
        search!(
            "@internal-core",
            $fetcher,
            $query,
            $page,
            $fetcher.search_res.1.artist,
            2,
            super::ArtistUnit
        )
    };

    ("@internal-core", $fetcher: expr, $query: expr, $page: expr, $store_target: expr, $filter_index: expr, $unit_type: ty) => {{
        let suffix = format!(
            "/search?q={query}&type={s_type}&{region}&page={page}&{fields}",
            query = $query,
            s_type = FILTER_TYPE[$filter_index],
            region = REGION,
            fields = FIELDS[$filter_index],
            page = $page
        );
        let lower_limit = $page * ITEM_PER_PAGE;
        let mut upper_limit = std::cmp::min($store_target.len(), lower_limit + ITEM_PER_PAGE);

        let is_new_query = *$query != $fetcher.search_res.0;
        let is_new_type = $fetcher.search_res.1.last_fetched != $filter_index;
        let insufficient_data = upper_limit.checked_sub(lower_limit).unwrap_or(0) < ITEM_PER_PAGE;

        $fetcher.search_res.1.last_fetched = $filter_index;
        if is_new_query || insufficient_data || is_new_type {
            let obj = $fetcher.send_request::<Vec<$unit_type>>(&suffix, 1).await;
            if is_new_query || is_new_type {
                $store_target.clear();
            }
            match obj {
                Ok(data) => {
                    $fetcher.search_res.0 = $query.to_string();
                    $store_target.extend_from_slice(data.as_slice());
                    upper_limit = std::cmp::min($store_target.len(), lower_limit + ITEM_PER_PAGE);
                }
                Err(e) => return Err(e),
            }
        }

        if upper_limit > lower_limit {
            Ok($store_target[lower_limit..upper_limit].to_vec())
        } else {
            Err(ReturnAction::EOR)
        }
    }};
}

impl Fetcher {
    async fn send_request<'de, Res>(
        &mut self,
        path: &str,
        retry_for: i32,
    ) -> Result<Res, ReturnAction>
    where
        Res: serde::de::DeserializeOwned,
    {
        let res = self
            .client
            .get(self.servers[self.active_server_index].to_string() + path)
            // .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
        match res {
            Ok(response) => {
                if let Ok(obj) = response.json::<Res>().await {
                    Ok(obj)
                } else {
                    Err(ReturnAction::Failed)
                }
            }
            Err(_) if retry_for > 0 => {
                self.change_server();
                Err(ReturnAction::Retry)
            }
            Err(_) => Err(ReturnAction::Failed),
        }
    }

    pub async fn get_trending_music(
        &mut self,
        page: usize,
    ) -> Result<&[super::MusicUnit], ReturnAction> {
        let lower_limit = ITEM_PER_PAGE * page;

        if self.trending_now.is_none() {
            let suffix = format!(
                "/trending?type=Music&{region}&{music_field}",
                region = REGION,
                music_field = FIELDS[0]
            );

            let obj = self.send_request::<Vec<super::MusicUnit>>(&suffix, 2).await;
            match obj {
                Ok(mut res) => {
                    res.shrink_to_fit();
                    self.trending_now = Some(res);
                }
                Err(e) => return Err(e),
            }
        }

        let trending_now = self.trending_now.as_ref().unwrap();
        let upper_limit = std::cmp::min(trending_now.len(), lower_limit + ITEM_PER_PAGE);

        if lower_limit >= upper_limit {
            Err(ReturnAction::EOR)
        } else {
            Ok(&trending_now[lower_limit..upper_limit])
        }
    }

    pub async fn get_playlist_content(
        &mut self,
        playlist_id: &str,
        page: usize,
    ) -> Result<Vec<super::MusicUnit>, ReturnAction> {
        let lower_limit = page * ITEM_PER_PAGE;

        let is_new_id = playlist_id != &self.playlist_content.0;
        if is_new_id || self.playlist_content.1.len() == 0 {
            self.playlist_content.0 = playlist_id.to_string();
            let suffix = format!(
                "/playlists/{playlist_id}?fields=videos",
                playlist_id = playlist_id
            );

            let obj = self.send_request::<Vec<super::MusicUnit>>(&suffix, 1).await;
            match obj {
                Ok(mut data) => {
                    data.shrink_to_fit();
                    self.playlist_content.1 = data;
                }
                Err(e) => return Err(e),
            }
        }

        let upper_limit = std::cmp::min(self.playlist_content.1.len(), lower_limit + ITEM_PER_PAGE);
        if lower_limit >= upper_limit {
            Err(ReturnAction::EOR)
        } else {
            let mut res = self.playlist_content.1[lower_limit..upper_limit].to_vec();
            res.shrink_to_fit();
            Ok(res)
        }
    }

    pub async fn search_music(
        &mut self,
        query: &str,
        page: usize,
    ) -> Result<Vec<super::MusicUnit>, ReturnAction> {
        search!("music", self, query, page)
    }

    pub async fn search_playlist(
        &mut self,
        query: &str,
        page: usize,
    ) -> Result<Vec<super::PlaylistUnit>, ReturnAction> {
        search!("playlist", self, query, page)
    }

    pub async fn search_artist(
        &mut self,
        query: &str,
        page: usize,
    ) -> Result<Vec<super::ArtistUnit>, ReturnAction> {
        search!("artist", self, query, page)
    }
}

// ------------- TEST ----------------
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn test_trending_extractor() {
        let mut fetcher = Fetcher::new();
        let mut page = 0;

        while let Ok(data) = fetcher.get_trending_music(page).await {
            println!("--------- Trending [{}] ----------", page);
            println!("{:#?}", data);
            page += 1;
        }
    }

    #[tokio::test]
    async fn check_format() {
        let sample_response = r#"{
                                    "title": "Some song title",
                                    "videoId": "WNgO6G7uERU",
                                    "author": "CHHEWANG",
                                    "lengthSeconds": 271
                                }"#;
        let obj: super::super::MusicUnit = serde_json::from_str(sample_response).unwrap();
        assert_eq!(
            obj,
            super::super::MusicUnit {
                liked: false,
                artist: "CHHEWANG".to_string(),
                name: "Some song title".to_string(),
                duration: "4:31".to_string(),
                path: "https://www.youtube.com/watch?v=WNgO6G7uERU".to_string(),
            },
        );
    }
    #[tokio::test]
    async fn check_music_search() {
        let mut fetcher = Fetcher::new();
        let obj = fetcher.search_music("Bartika Eam Rai", 1).await;
        eprintln!("{:#?}", obj);
    }

    #[tokio::test]
    async fn check_playlist_search() {
        let mut fetcher = Fetcher::new();
        let obj = fetcher.search_playlist("Spotify Chill mix", 1).await;
        eprintln!("{:#?}", obj);
    }

    #[tokio::test]
    async fn check_artist_search() {
        let mut fetcher = Fetcher::new();
        let obj = fetcher.search_artist("Rachana Dahal", 1).await;
        eprintln!("{:#?}", obj);
    }

    #[tokio::test]
    async fn check_playlist_content() {
        let mut fetcher = Fetcher::new();
        let obj = fetcher
            .get_playlist_content("PLo4CR7vlB7oIokHy6JOnPLAmSiilJq7ms", 1)
            .await;
        eprintln!("{:#?}", obj);
    }
}
