use serde::{Deserialize, Serialize};
use serde_json::Value;
use wry::Result;
use wry::{Application, Attributes, RpcResponse};

#[derive(Debug, Serialize, Deserialize)]
struct MessageParameters {
    message: String,
}

fn main() -> Result<()> {
    let mut app = Application::new()?;

    let html = r#"
<script>
let fullscreen = false;
async function toggleFullScreen() {
    await rpc.call('fullscreen', !fullscreen);
    fullscreen = !fullscreen;
}

async function getAsyncRpcResult() {
    const reply = await rpc.call('send-parameters', {'message': 'WRY'});
    const result = document.getElementById('rpc-result');
    result.innerText = reply;
}

</script>
<div><button onclick="toggleFullScreen();">Toggle fullscreen</button></div>
<div><button onclick="getAsyncRpcResult();">Send parameters</button></div>
<div id="rpc-result"></div>
"#;

    let markup = urlencoding::encode(html);
    let attributes = Attributes {
        url: Some(format!("data:text/html,{}", markup)),
        ..Default::default()
    };

    let proxy = app.add_window_with_configs(
        attributes,
        None,
        None,
        Some(Box::new(move |mut req| {
            let mut response = None;
            if &req.method == "fullscreen" {
                if let Some(params) = req.params.take() {
                    if let Some(mut args) = serde_json::from_value::<Vec<bool>>(params).ok() {
                        if args.len() > 0 {
                            let flag = args.swap_remove(0);
                            // NOTE: in the real world we need to reply with an error
                            //let _ = proxy.set_fullscreen(flag);
                        };
                        response = Some(RpcResponse::new_result(req.id.take(), None));
                    }
                }
            } else if &req.method == "send-parameters" {
                if let Some(params) = req.params.take() {
                    if let Some(mut args) =
                        serde_json::from_value::<Vec<MessageParameters>>(params).ok()
                    {
                        let result = if args.len() > 0 {
                            let msg = args.swap_remove(0);
                            Some(Value::String(format!("Hello, {}!", msg.message)))
                        } else {
                            // NOTE: in the real-world we should send an error response here!
                            None
                        };
                        // Must always send a response as this is a `call()`
                        response = Some(RpcResponse::new_result(req.id.take(), result));
                    }
                }
            }

            response
        })),
    )?;

    app.run();
    Ok(())
}
