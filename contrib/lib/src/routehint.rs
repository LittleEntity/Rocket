use difference::Difference;
use futures::lock::Mutex;
use rocket::http;
use rocket::{
    fairing::{Fairing, Info, Kind},
    figment::value::UncasedStr,
};
use rocket::{Data, Request, Rocket, Route};
use yansi::Color;

pub struct RouteHint {
    routes: Mutex<Vec<rocket::Route>>,
}

impl RouteHint {
    pub fn new() -> Self {
        Self {
            routes: Mutex::new(Vec::new()),
        }
    }
}

#[derive(Debug)]
enum MethodDiff {
    Same(http::Method),
    Change(http::Method, http::Method),
}

#[derive(Debug)]
enum SegmentDiff {
    StaticMatch(String),
    SingleMatch(String, String),
    MultiMatch(String, Vec<String>),
    Diff(Vec<Difference>),
    Missing(String),
    Unexpected(String),
}

#[derive(Debug)]
enum MediaTypeDiff {
    IsMatch {
        top: IsMediaTypeMatch,
        sub: IsMediaTypeMatch,
    },
    Missing(String),
    Unexpected(String),
    None,
}

#[derive(Debug)]
enum IsMediaTypeMatch {
    TrueStatic(String),
    TrueDynamic(String, String),
    False(Vec<Difference>),
}

impl IsMediaTypeMatch {
    fn from(route_mt_part: &UncasedStr, req_mt_part: &UncasedStr) -> IsMediaTypeMatch {
        use IsMediaTypeMatch::*;
        if route_mt_part == req_mt_part {
            TrueStatic(route_mt_part.to_string())
        } else if route_mt_part == "*" || req_mt_part == "*" {
            TrueDynamic(route_mt_part.to_string(), req_mt_part.to_string())
        } else {
            False(
                difference::Changeset::new(req_mt_part.as_str(), route_mt_part.as_str(), "").diffs,
            )
        }
    }
}

#[derive(Debug)]
struct RoutingDiff {
    method: MethodDiff,
    path_diff: Vec<SegmentDiff>,
    query: Vec<SegmentDiff>,
    media_type: MediaTypeDiff,
}

impl RoutingDiff {
    fn from(route: &Route, request: &Request) -> Self {
        RoutingDiff {
            method: if route.method == request.method() {
                MethodDiff::Same(route.method)
            } else {
                MethodDiff::Change(route.method, request.method())
            },
            path_diff: Self::path_diff(route, request),
            query: Self::query_diff(route, request),
            media_type: Self::media_type_diff(route, request),
        }
    }

    fn media_type_diff(route: &Route, request: &Request) -> MediaTypeDiff {
        if let Some(route_mt) = &route.format {
            if let Some(req_mt) = request.format() {
                MediaTypeDiff::IsMatch {
                    top: IsMediaTypeMatch::from(route_mt.top(), req_mt.top()),
                    sub: IsMediaTypeMatch::from(route_mt.sub(), req_mt.sub()),
                }
            } else {
                MediaTypeDiff::Missing(route_mt.to_string())
            }
        } else if let Some(req_mt) = request.format() {
            MediaTypeDiff::Unexpected(req_mt.to_string())
        } else {
            MediaTypeDiff::None
        }
    }

    fn query_diff(route: &Route, request: &Request) -> Vec<SegmentDiff> {
        if let Some(route_params) = &route.metadata().query_segments {
            if let Some(req_params) = request.raw_query_items() {
                // when there are parameters expected, but more paramters given than expected: it is difficult to decide which one is unexpected, when others are mismatched. Therefore we simply do not log unexpected ones in that case.
                let mut matched_req_params = Vec::new();
                route_params
                    .iter()
                    .map(|route_param| {
                        use http::route::Kind::*;
                        match route_param.kind {
                            Static => {
                                if let Some(req_param) = req_params
                                    .clone()
                                    .find(|req_param| route_param.string == req_param.raw.as_str())
                                {
                                    matched_req_params.push(req_param);
                                    SegmentDiff::StaticMatch(req_param.raw.to_string())
                                } else {
                                    SegmentDiff::Diff(
                                        req_params
                                            .clone()
                                            .map(|req_param| {
                                                difference::Changeset::new(
                                                    req_param.raw.as_str(),
                                                    &route_param.string,
                                                    "",
                                                )
                                            })
                                            .min_by(|a, b| a.distance.cmp(&b.distance))
                                            .expect("req_params must not be empty")
                                            .diffs,
                                    )
                                }
                            }
                            Single => {
                                if let Some(req_param) = req_params
                                    .clone()
                                    .find(|req_param| req_param.key.as_str() == route_param.name)
                                {
                                    matched_req_params.push(req_param);
                                    SegmentDiff::SingleMatch(
                                        route_param.string.to_string(),
                                        req_param.raw.as_str().into(),
                                    )
                                } else {
                                    // beware: this could be Optional. So this could be valid. If it is an error, it is logged later by router code.
                                    SegmentDiff::Missing(route_param.string.to_string())
                                }
                            }
                            Multi => SegmentDiff::MultiMatch(
                                route_param.string.to_string(),
                                req_params
                                    .clone()
                                    .filter(|req_param| {
                                        matched_req_params
                                            .iter()
                                            .all(|matched_req_param| req_param != matched_req_param)
                                    })
                                    .map(|req_param| req_param.raw.to_string())
                                    .collect(),
                            ),
                        }
                    })
                    .collect()
            } else {
                // has route query params
                // does not have request query params
                route_params
                    .iter()
                    .map(|route_param| SegmentDiff::Missing(route_param.string.to_string()))
                    .collect()
            }
        } else {
            // does not have route query params
            if let Some(req_params) = request.raw_query_items() {
                req_params
                    .map(|req_param| SegmentDiff::Unexpected(req_param.raw.to_string()))
                    .collect()
            } else {
                vec![]
            }
        }
    }

    fn path_diff(route: &Route, request: &Request) -> Vec<SegmentDiff> {
        let route_segments = &route.metadata().path_segments;
        let mut request_segments = request.uri().segments();
        let mut result = Vec::new();
        for route_seg in route_segments {
            result.push(if let Some(req_seg) = request_segments.next() {
                use http::route::Kind::*;
                match route_seg.kind {
                    Static => {
                        if route_seg.string == req_seg {
                            SegmentDiff::StaticMatch(req_seg.into())
                        } else {
                            SegmentDiff::Diff(
                                difference::Changeset::new(req_seg, &route_seg.string, "").diffs,
                            )
                        }
                    }
                    Single => {
                        SegmentDiff::SingleMatch(route_seg.string.to_string(), req_seg.into())
                    }
                    Multi => SegmentDiff::MultiMatch(
                        route_seg.string.to_string(),
                        std::iter::once(req_seg)
                            .chain(request_segments.clone())
                            .map(|s| s.into())
                            .collect(),
                    ),
                }
            } else {
                SegmentDiff::Missing(route_seg.string.to_string())
            });
        }
        if !matches!(result.last(), Some(SegmentDiff::MultiMatch(_, _))) {
            if let Some(seg) = request_segments.next() {
                std::iter::once(seg)
                    .chain(request_segments)
                    .map(|s| s.into())
                    .for_each(|seg| result.push(SegmentDiff::Unexpected(seg)))
            }
        }
        result
    }

    fn color_add_diffs(diffs: &Vec<Difference>, color: &Color) -> String {
        diffs.iter().fold(String::new(), |acc, diff| match diff {
            Difference::Same(s) => acc + s,
            Difference::Add(s) => acc + &format!("{}", color.paint(s)),
            Difference::Rem(_) => acc,
        })
    }

    fn color_rem_diffs(diffs: &Vec<Difference>, color: &Color) {}

    fn print(&self) {
        let red = Color::RGB(179, 0, 0);
        let green = Color::RGB(0, 128, 0);

        let route_method = match self.method {
            MethodDiff::Same(m) => m.to_string(),
            MethodDiff::Change(route_m, _request_m) => {
                format!("{}", green.paint(route_m.to_string()))
            }
        };
        let request_method = match self.method {
            MethodDiff::Same(m) => m.to_string(),
            MethodDiff::Change(_route_m, request_m) => {
                format!("{}", red.paint(request_m.to_string()))
            }
        };

        let mut route_path =
            self.path_diff
                .iter()
                .fold(String::new(), |acc, seg_diff| match seg_diff {
                    SegmentDiff::StaticMatch(route_seg)
                    | SegmentDiff::SingleMatch(route_seg, _)
                    | SegmentDiff::MultiMatch(route_seg, _) => acc + "/" + route_seg,
                    SegmentDiff::Diff(diffs) => acc + "/" + &Self::color_add_diffs(diffs, &green),
                    SegmentDiff::Missing(route_seg) => {
                        acc + "/" + &format!("{}", green.paint(route_seg))
                    }
                    SegmentDiff::Unexpected(_) => acc,
                });
        if route_path.len() == 0 {
            route_path.push('/')
        }

        let mut request_path =
            self.path_diff
                .iter()
                .fold(String::new(), |acc, seg_diff| match seg_diff {
                    SegmentDiff::StaticMatch(req_seg) | SegmentDiff::SingleMatch(_, req_seg) => {
                        acc + "/" + req_seg
                    }
                    SegmentDiff::MultiMatch(_route_seg, req_segs) => {
                        acc + &req_segs
                            .iter()
                            .fold(String::new(), |acc, req_seg| acc + "/" + req_seg)
                    }
                    SegmentDiff::Diff(diffs) => {
                        acc + "/"
                            + &diffs.iter().fold(String::new(), |acc, diff| match diff {
                                Difference::Same(s) => acc + s,
                                Difference::Add(_route_part) => acc,
                                Difference::Rem(req_part) => {
                                    acc + &format!("{}", red.paint(req_part))
                                }
                            })
                    }
                    SegmentDiff::Missing(_route_seg) => acc,
                    SegmentDiff::Unexpected(req_seg) => {
                        acc + "/" + &format!("{}", red.paint(req_seg))
                    }
                });
        if request_path.len() == 0 {
            request_path.push('/')
        }

        let mut route_query =
            self.query
                .iter()
                .fold(String::new(), |acc, seg_diff| match seg_diff {
                    SegmentDiff::StaticMatch(route_seg)
                    | SegmentDiff::SingleMatch(route_seg, _)
                    | SegmentDiff::MultiMatch(route_seg, _) => acc + "&" + route_seg,
                    SegmentDiff::Diff(diffs) => {
                        acc + "&"
                            + &diffs
                                .iter()
                                .fold(String::new(), |seg_diff, diff| match diff {
                                    Difference::Same(s) => seg_diff + s,
                                    Difference::Add(route_part) => {
                                        seg_diff + &format!("{}", green.paint(route_part))
                                    }
                                    Difference::Rem(_req_part) => seg_diff,
                                })
                    }
                    SegmentDiff::Missing(route_seg) => {
                        acc + "&" + &format!("{}", green.paint(route_seg))
                    }
                    SegmentDiff::Unexpected(_) => acc,
                });
        if route_query.len() > 0 {
            route_query.replace_range(0..1, "?");
        }

        let mut request_query =
            self.query
                .iter()
                .fold(String::new(), |acc, seg_diff| match seg_diff {
                    SegmentDiff::StaticMatch(req_seg) | SegmentDiff::SingleMatch(_, req_seg) => {
                        acc + "&" + req_seg
                    }
                    SegmentDiff::MultiMatch(_route_seg, req_segs) => {
                        acc + &req_segs
                            .iter()
                            .fold(String::new(), |acc, req_seg| acc + "&" + req_seg)
                    }
                    SegmentDiff::Diff(diffs) => {
                        acc + "&"
                            + &diffs.iter().fold(String::new(), |acc, diff| match diff {
                                Difference::Same(s) => acc + s,
                                Difference::Add(_route_part) => acc,
                                Difference::Rem(req_part) => {
                                    acc + &format!("{}", red.paint(req_part))
                                }
                            })
                    }
                    SegmentDiff::Missing(_route_seg) => acc,
                    SegmentDiff::Unexpected(req_seg) => {
                        acc + "&" + &format!("{}", red.paint(req_seg))
                    }
                });
        if request_query.len() > 0 {
            request_query.replace_range(0..1, "?");
        }

        let route_media = match &self.media_type {
            MediaTypeDiff::IsMatch { top, sub } => match top {
                IsMediaTypeMatch::TrueStatic(route_mt) => route_mt.clone(),
                IsMediaTypeMatch::TrueDynamic(route_mt, _) => route_mt.clone(),
                IsMediaTypeMatch::False(diffs) => {
                    diffs.iter().fold(String::new(), |acc, diff| match diff {
                        Difference::Same(s) => acc + s,
                        Difference::Add(route_mt_part) => {
                            acc + &format!("{}", green.paint(route_mt_part))
                        }
                        Difference::Rem(_req_mt_part) => acc,
                    })
                }
            },
            MediaTypeDiff::Missing(route_mt) => format!("{}", green.paint(&route_mt)),
            MediaTypeDiff::Unexpected(req_mt) => "".into(),
            MediaTypeDiff::None => "".into(),
        };

        println!(
            "{}: {}{}   {}",
            route_method, route_path, route_query, route_media
        );
        println!("{}: {}{}", request_method, request_path, request_query);
    }
}

#[rocket::async_trait]
impl Fairing for RouteHint {
    fn info(&self) -> Info {
        Info {
            name: "routehinter",
            kind: Kind::Attach | Kind::Launch | Kind::Request,
        }
    }

    async fn on_attach(&self, rocket: Rocket) -> Result<Rocket, Rocket> {
        for route in rocket.routes() {
            self.routes.lock().await.push(route.clone());
        }
        Ok(rocket)
    }

    // unfortunately we cannot use on_launch, because it is not async
    // fn on_launch(&self, rocket: &Rocket) {
    //     for route in rocket.routes() {
    //         self.routes.lock().await.push(route.clone());
    //     }
    // }

    async fn on_request(&self, request: &mut Request<'_>, _data: &mut Data) {
        println!();
        println!("trying to match {}", request.uri());
        println!();
        for route in self.routes.lock().await.iter() {
            let routing_diff = RoutingDiff::from(route, request);
            routing_diff.print();
            // println!("    route: {}", route.uri);
            // println!("{:?}", routing_diff);
            println!();
        }
    }
}
