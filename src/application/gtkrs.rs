// Copyright 2019-2021 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use crate::{
  application::{InnerWebViewAttributes, InnerWindowAttributes},
  ApplicationProxy, Attributes, CustomProtocol, Error, Event as WryEvent, Icon, Message, Result,
  WebView, WebViewBuilder, WindowEvent as WryWindowEvent, WindowFileDropHandler, WindowMessage,
  WindowProxy, WindowRpcHandler,
};

use std::{
  cell::RefCell,
  collections::HashMap,
  rc::Rc,
  sync::{
    mpsc::{channel, Receiver, Sender},
    Arc, Mutex,
  },
};

use async_channel::unbounded;
use cairo::Operator;
use gio::{ApplicationExt as GioApplicationExt, Cancellable};
use gtk::{
  Application as GtkApp, ApplicationWindow, ApplicationWindowExt, GtkWindowExt, Inhibit, WidgetExt,
};

pub type WindowId = u32;

struct EventLoopProxy(async_channel::Sender<Message>);

impl Clone for EventLoopProxy {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}

#[derive(Clone)]
pub struct InnerApplicationProxy {
  proxy: EventLoopProxy,
  receiver: Arc<Mutex<Receiver<WryEvent>>>,
}

impl InnerApplicationProxy {
  pub fn send_message(&self, message: Message) -> Result<()> {
    self
      .proxy
      .0
      .try_send(message)
      .map_err(|_| Error::MessageSender)?;
    Ok(())
  }

  pub fn add_window(
    &self,
    attributes: Attributes,
    file_drop_handler: Option<WindowFileDropHandler>,
    rpc_handler: Option<WindowRpcHandler>,
    custom_protocols: Vec<CustomProtocol>,
  ) -> Result<WindowId> {
    let (sender, receiver): (Sender<WindowId>, Receiver<WindowId>) = channel();
    self.send_message(Message::NewWindow(
      attributes,
      sender,
      file_drop_handler,
      rpc_handler,
      custom_protocols,
    ))?;
    Ok(receiver.recv()?)
  }

  pub fn listen_event(&self) -> Result<WryEvent> {
    let rx = self.receiver.lock().unwrap();
    Ok(rx.recv()?)
  }
}

pub struct InnerApplication {
  webviews: HashMap<u32, WebView>,
  app: GtkApp,
  event_loop_proxy: EventLoopProxy,
  event_loop_proxy_rx: async_channel::Receiver<Message>,
  event_channel: (Sender<WryEvent>, Arc<Mutex<Receiver<WryEvent>>>),
}

impl InnerApplication {
  pub fn new() -> Result<Self> {
    let app = GtkApp::new(None, Default::default())?;
    let cancellable: Option<&Cancellable> = None;
    app.register(cancellable)?;

    let (event_loop_proxy_tx, event_loop_proxy_rx) = unbounded::<Message>();
    let (tx, rx) = channel();

    Ok(Self {
      webviews: HashMap::new(),
      app,
      event_loop_proxy: EventLoopProxy(event_loop_proxy_tx),
      event_loop_proxy_rx,
      event_channel: (tx, Arc::new(Mutex::new(rx))),
    })
  }

  pub fn create_webview(
    &mut self,
    attributes: Attributes,
    file_drop_handler: Option<WindowFileDropHandler>,
    rpc_handler: Option<WindowRpcHandler>,
    custom_protocols: Vec<CustomProtocol>,
  ) -> Result<u32> {
    let (window_attrs, webview_attrs) = attributes.split();
    let window = _create_window(&self.app, window_attrs)?;

    let webview = _create_webview(
      self.application_proxy(),
      window,
      custom_protocols,
      rpc_handler,
      file_drop_handler,
      webview_attrs,
    )?;

    let id = webview.window().get_id();
    self.webviews.insert(id, webview);

    Ok(id)
  }

  pub fn application_proxy(&self) -> InnerApplicationProxy {
    InnerApplicationProxy {
      proxy: self.event_loop_proxy.clone(),
      receiver: self.event_channel.1.clone(),
    }
  }

  pub fn run(self) {
    let proxy = self.application_proxy();
    let shared_webviews = Rc::new(RefCell::new(self.webviews));
    let shared_webviews_ = shared_webviews.clone();
    let event_sender = self.event_channel.0;

    {
      let webviews = shared_webviews.borrow();
      for (id, w) in webviews.iter() {
        let shared_webviews_ = shared_webviews_.clone();
        let id_ = *id;
        let tx_clone = event_sender.clone();
        w.window().connect_delete_event(move |_window, _event| {
          shared_webviews_.borrow_mut().remove(&id_);
          let _ = tx_clone.send(WryEvent::WindowEvent {
            window_id: id_,
            event: WryWindowEvent::CloseRequested,
          });
          Inhibit(false)
        });
      }
    }

    // Acquire the main context and set it as the current thread-default
    // Local futures must be spawned on the thread owning the main context
    let default_context = glib::MainContext::default();
    default_context.push_thread_default();
    default_context.spawn_local(process_messages(
      self.app,
      proxy,
      shared_webviews_,
      self.event_loop_proxy_rx,
    ));
    default_context.pop_thread_default();

    loop {
      {
        let webviews = shared_webviews.borrow_mut();

        if webviews.is_empty() {
          break;
        }

        for (_, w) in webviews.iter() {
          let _ = w.evaluate_script();
        }
      }

      gtk::main_iteration();
    }
  }
}

async fn process_messages(
  app: gtk::Application,
  proxy: InnerApplicationProxy,
  shared_webviews: Rc<RefCell<HashMap<u32, WebView>>>,
  event_loop_proxy_rx: async_channel::Receiver<Message>,
) {
  while let Ok(message) = event_loop_proxy_rx.recv().await {
    match message {
      Message::NewWindow(attributes, sender, file_drop_handler, rpc_handler, custom_protocols) => {
        let (window_attrs, webview_attrs) = attributes.split();
        match _create_window(&app, window_attrs) {
          Ok(window) => {
            if let Err(e) = sender.send(window.get_id()) {
              log::error!("{}", e);
            }
            match _create_webview(
              proxy.clone(),
              window,
              custom_protocols,
              rpc_handler,
              file_drop_handler,
              webview_attrs,
            ) {
              Ok(webview) => {
                let id = webview.window().get_id();
                let shared_webviews_ = shared_webviews.clone();
                webview
                  .window()
                  .connect_delete_event(move |_window, _event| {
                    shared_webviews_.borrow_mut().remove(&id);
                    Inhibit(false)
                  });
                let mut webviews = shared_webviews.borrow_mut();
                webviews.insert(id, webview);
              }
              Err(e) => {
                log::error!("{}", e);
              }
            }
          }
          Err(e) => {
            log::error!("{}", e);
          }
        }
      }
      Message::Window(id, window_message) => {
        if let Some(webview) = shared_webviews.borrow_mut().get_mut(&id) {
          let window = webview.window();
          match window_message {
            WindowMessage::SetResizable(resizable) => {
              window.set_resizable(resizable);
            }
            WindowMessage::SetTitle(title) => window.set_title(&title),
            WindowMessage::Maximize => {
              window.maximize();
            }
            WindowMessage::Unmaximize => {
              window.unmaximize();
            }
            WindowMessage::Minimize => {
              window.iconify();
            }
            WindowMessage::Unminimize => {
              window.deiconify();
            }
            WindowMessage::Show => {
              window.show_all();
            }
            WindowMessage::Hide => {
              window.hide();
            }
            WindowMessage::Close => {
              window.close();
            }
            WindowMessage::SetDecorations(decorations) => {
              window.set_decorated(decorations);
            }
            WindowMessage::SetAlwaysOnTop(always_on_top) => {
              window.set_keep_above(always_on_top);
            }
            WindowMessage::SetWidth(width) => {
              window.resize(width as i32, window.get_size().1);
            }
            WindowMessage::SetHeight(height) => {
              window.resize(window.get_size().0, height as i32);
            }
            WindowMessage::Resize { width, height } => {
              window.resize(width as i32, height as i32);
            }
            WindowMessage::SetMinSize {
              min_width,
              min_height,
            } => {
              window.set_geometry_hints::<ApplicationWindow>(
                None,
                Some(&gdk::Geometry {
                  min_width: min_width as i32,
                  min_height: min_height as i32,
                  max_width: 0,
                  max_height: 0,
                  base_width: 0,
                  base_height: 0,
                  width_inc: 0,
                  height_inc: 0,
                  min_aspect: 0f64,
                  max_aspect: 0f64,
                  win_gravity: gdk::Gravity::Center,
                }),
                gdk::WindowHints::MIN_SIZE,
              );
            }
            WindowMessage::SetMaxSize {
              max_width,
              max_height,
            } => {
              window.set_geometry_hints::<ApplicationWindow>(
                None,
                Some(&gdk::Geometry {
                  min_width: 0,
                  min_height: 0,
                  max_width: max_width as i32,
                  max_height: max_height as i32,
                  base_width: 0,
                  base_height: 0,
                  width_inc: 0,
                  height_inc: 0,
                  min_aspect: 0f64,
                  max_aspect: 0f64,
                  win_gravity: gdk::Gravity::Center,
                }),
                gdk::WindowHints::MAX_SIZE,
              );
            }
            WindowMessage::SetX(x) => {
              let (_, y) = window.get_position();
              window.move_(x as i32, y);
            }
            WindowMessage::SetY(y) => {
              let (x, _) = window.get_position();
              window.move_(x, y as i32);
            }
            WindowMessage::SetPosition { x, y } => {
              window.move_(x as i32, y as i32);
            }
            WindowMessage::SetFullscreen(fullscreen) => {
              if fullscreen {
                window.fullscreen();
              } else {
                window.unfullscreen();
              }
            }
            WindowMessage::SetIcon(icon) => {
              if let Ok(icon) = load_icon(icon) {
                window.set_icon(Some(&icon));
              }
            }
            WindowMessage::EvaluationScript(script) => {
              let _ = webview.dispatch_script(&script);
            }
            WindowMessage::BeginDrag { x, y } => {
              window.begin_move_drag(1, x as i32, y as i32, 0);
            }
            WindowMessage::IsMaximized(tx) => {
              tx.send(window.get_property_is_maximized()).ok();
            }
          }
        }
      }
    }
  }
}

fn load_icon(icon: Icon) -> Result<gdk_pixbuf::Pixbuf> {
  let image = image::load_from_memory(&icon.0)?.into_rgba8();
  let (width, height) = image.dimensions();
  let row_stride = image.sample_layout().height_stride;
  Ok(gdk_pixbuf::Pixbuf::from_mut_slice(
    image.into_raw(),
    gdk_pixbuf::Colorspace::Rgb,
    true,
    8,
    width as i32,
    height as i32,
    row_stride as i32,
  ))
}

fn _create_window(app: &GtkApp, attributes: InnerWindowAttributes) -> Result<ApplicationWindow> {
  let window = ApplicationWindow::new(app);

  window.set_geometry_hints::<ApplicationWindow>(
    None,
    Some(&gdk::Geometry {
      min_width: attributes.min_width.unwrap_or_default() as i32,
      min_height: attributes.min_height.unwrap_or_default() as i32,
      max_width: attributes.max_width.unwrap_or_default() as i32,
      max_height: attributes.max_height.unwrap_or_default() as i32,
      base_width: 0,
      base_height: 0,
      width_inc: 0,
      height_inc: 0,
      min_aspect: 0f64,
      max_aspect: 0f64,
      win_gravity: gdk::Gravity::Center,
    }),
    (if attributes.min_width.is_some() || attributes.min_height.is_some() {
      gdk::WindowHints::MIN_SIZE
    } else {
      gdk::WindowHints::empty()
    }) | (if attributes.max_width.is_some() || attributes.max_height.is_some() {
      gdk::WindowHints::MAX_SIZE
    } else {
      gdk::WindowHints::empty()
    }),
  );

  if attributes.resizable {
    window.set_default_size(attributes.width as i32, attributes.height as i32);
  } else {
    window.set_size_request(attributes.width as i32, attributes.height as i32);
  }

  if attributes.transparent {
    if let Some(screen) = window.get_screen() {
      if let Some(visual) = screen.get_rgba_visual() {
        window.set_visual(Some(&visual));
      }
    }

    window.connect_draw(|_, cr| {
      cr.set_source_rgba(0., 0., 0., 0.);
      cr.set_operator(Operator::Source);
      cr.paint();
      cr.set_operator(Operator::Over);
      Inhibit(false)
    });
    window.set_app_paintable(true);
  }

  window.set_skip_taskbar_hint(attributes.skip_taskbar);
  window.set_resizable(attributes.resizable);
  window.set_title(&attributes.title);
  if attributes.maximized {
    window.maximize();
  }
  window.set_visible(attributes.visible);
  window.set_decorated(attributes.decorations);
  window.set_keep_above(attributes.always_on_top);

  match (attributes.x, attributes.y) {
    (Some(x), Some(y)) => window.move_(x as i32, y as i32),
    _ => {}
  }

  if attributes.fullscreen {
    window.fullscreen();
  }
  if let Some(icon) = attributes.icon {
    window.set_icon(Some(&load_icon(icon)?));
  }

  Ok(window)
}

fn _create_webview(
  proxy: InnerApplicationProxy,
  window: ApplicationWindow,
  custom_protocols: Vec<CustomProtocol>,
  rpc_handler: Option<WindowRpcHandler>,
  file_drop_handler: Option<WindowFileDropHandler>,

  attributes: InnerWebViewAttributes,
) -> Result<WebView> {
  let window_id = window.get_id();
  let mut webview = WebViewBuilder::new(window)?.transparent(attributes.transparent);
  for js in attributes.initialization_scripts {
    webview = webview.initialize_script(&js);
  }

  webview = match attributes.url {
    Some(url) => webview.load_url(&url)?,
    None => webview,
  };

  for protocol in custom_protocols {
    webview = webview.register_protocol(protocol.name, protocol.handler);
  }

  let proxy_ = proxy.clone();
  webview = webview.set_rpc_handler(Box::new(move |request| {
    let proxy = WindowProxy::new(
      ApplicationProxy {
        inner: proxy_.clone(),
      },
      window_id,
    );

    if &request.method == "__WRY_BEGIN_WINDOW_DRAG__" {
      if let Some(params) = &request.params {
        let x = params[0].as_f64()?;
        let y = params[1].as_f64()?;
        proxy.begin_drag(x, y).unwrap();
      }
    }

    if let Some(rpc_handler) = &rpc_handler {
      rpc_handler(proxy, request)
    } else {
      None
    }
  }));

  webview = webview.set_file_drop_handler(Some(Box::new(move |event| {
    let proxy = WindowProxy::new(
      ApplicationProxy {
        inner: proxy.clone(),
      },
      window_id,
    );

    if let Some(file_drop_handler) = &file_drop_handler {
      file_drop_handler(proxy, event)
    } else {
      false
    }
  })));

  let webview = webview.build()?;
  Ok(webview)
}
