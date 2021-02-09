use rocket::{
    get, http::ContentType, post, request::Form, response::Content, routes, FromForm, Rocket,
};
use rocket_contrib::routehint::RouteHint;
use std::path::PathBuf;

#[rocket::main]
async fn main() {
    rocket().launch().await.expect("server launched");
}

fn rocket() -> Rocket {
    rocket::ignite()
        .mount(
            "/",
            routes!(
                get_hello,
                get_index,
                get_guide,
                get_hello_flag,
                get_path_and_query,
                get_some_html,
                get_form,
                post_form,
            ),
        )
        .attach(RouteHint::new())
}

#[get("/hello/<name>/mood/<mood>")]
fn get_hello(name: String, mood: String) -> String {
    format!("Hello {} {}!", mood, name)
}

#[get("/hello?flag=23&lalelu=1")]
fn get_hello_flag() -> &'static str {
    "Hello Flag!"
}

#[get("/")]
fn get_index() -> &'static str {
    "Welcome Visitor!"
}

// todo: does Rocket evaluate q-factor weighting?
// calling this in firefox leads to 404. Though firefox accepts with */*;q=0.8
// https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Accept
// https://developer.mozilla.org/en-US/docs/Glossary/Quality_values
#[get("/something.txt", format = "text/plain")]
fn get_some_html() -> &'static str {
    "oh .. u r specific"
}

#[get("/guide/<_topic..>")]
fn get_guide(_topic: PathBuf) -> &'static str {
    "Welcome to the Guide Rustacian!"
}

#[derive(FromForm, Debug)]
struct UserInput {
    user_input: u32,
}

#[get("/form")]
fn get_form() -> Content<&'static str> {
    Content(
        ContentType::HTML,
        "
        <!DOCTYPE html>
        <html lang='en'>
        <head>
            <title>Form</title>
        </head>
        <body>
            <form method='post'>
                <input type='text' name='user_input'>
                <button type='submit'>submit</button>
            </form>
        </body>
        </html>
        ",
    )
}

#[post("/form", data = "<user_input>")]
fn post_form(user_input: Form<UserInput>) -> String {
    format!("{:?}", user_input)
}

#[derive(Debug)]
struct MyQuery<'q> {
    params: Vec<&'q rocket::http::RawStr>,
}

impl<'q> rocket::request::FromQuery<'q> for MyQuery<'q> {
    type Error = ();

    fn from_query(query: rocket::request::Query<'q>) -> Result<Self, Self::Error> {
        Ok(MyQuery {
            params: query.map(|param| param.raw).collect(),
        })
    }
}

#[get("/<path..>?<when>&<query..>")]
fn get_path_and_query(path: PathBuf, query: MyQuery, when: String) -> String {
    format!("path: {:?} query: {:?} when: {:?}", path, query, when)
}
