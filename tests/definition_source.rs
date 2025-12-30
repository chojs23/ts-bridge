use lsp_types::{
    GotoDefinitionResponse,
    request::{GotoDefinition, Request},
};
use serde_json::json;

use ts_bridge::protocol;

#[test]
fn source_definition_locations_convert_to_location_links() {
    // Simulate Neovim asking for `textDocument/definition` with the custom
    // `context.sourceDefinition` flag enabled.
    let params = json!({
        "textDocument": { "uri": "file:///workspace/app.ts" },
        "position": { "line": 4, "character": 2 },
        "context": { "sourceDefinition": true }
    });

    let spec = protocol::route_request(GotoDefinition::METHOD, params)
        .expect("definition request should route");
    assert_eq!(
        spec.payload.get("command"),
        Some(&json!("findSourceDefinition")),
        "handler must forward to findSourceDefinition when Neovim opts in"
    );

    // Pretend tsserver responds with a single span so we can run the adapter end
    // of the pipeline. This mimics Neovim receiving a LocationLink array.
    let tsserver_payload = json!({
        "command": "findSourceDefinition",
        "body": [{
            "file": "/workspace/lib.ts",
            "start": { "line": 6, "offset": 3 },
            "end": { "line": 6, "offset": 10 }
        }]
    });
    let adapter = spec.on_response.expect("definition adapter present");
    let value = adapter(&tsserver_payload, spec.response_context.as_ref())
        .expect("definition adapter should convert response");
    match serde_json::from_value::<GotoDefinitionResponse>(value)
        .expect("adapter must emit a valid LSP response")
    {
        GotoDefinitionResponse::Link(links) => {
            assert_eq!(links.len(), 1, "Neovim expects LocationLinks");
            let link = &links[0];
            assert_eq!(
                link.target_uri.to_string(),
                "file:///workspace/lib.ts",
                "LocationLink target must match the tsserver file span"
            );
        }
        other => panic!("expected LocationLink response, got {other:?}"),
    }
}
