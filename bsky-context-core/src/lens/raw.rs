use crate::model::ContextWeb;

pub(super) fn render(web: &ContextWeb) -> String {
    web.to_json_pretty()
}
