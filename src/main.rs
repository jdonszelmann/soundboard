use std::collections::HashMap;
use std::io::{Cursor, stdout,};
use std::thread;
use color_eyre::eyre::Context;
use crossterm::{event, ExecutableCommand};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::{Frame, Terminal};
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::prelude::Color;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use rodio::{Decoder, OutputStream, Sink};
use taffy::{AvailableSpace, Dimension, Display, LengthPercentage, MaxTrackSizingFunction, MinMax, MinTrackSizingFunction, NodeId, PrintTree, Size, TaffyTree, TrackSizingFunction, TraversePartialTree};
use taffy::GridTrackRepetition::{AutoFit};

macro_rules! data {
    ($name: literal $(,)?) => {
        ($name, include_bytes!(concat!("../assets/", $name, ".wav")))
    };

    ($name: literal, $ext: literal, $(,)?) => {
        ($name, include_bytes!(concat!("../assets/", $name, $ext)))
    };
}

type ConfigEntry = (KeyCode, (&'static str, &'static [u8]));

const CONFIG: &[ConfigEntry] = &[
    (KeyCode::Char('g'), data!("geen-grote-blij")),
    (KeyCode::Char('b'), data!("grote-blij")),
    (KeyCode::Char('p'), data!("puree")),
];

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?.execute(EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut last_computed_size = None;
    let mut mapping = HashMap::new();
    let (mut tree, root_node) = generate_taffy_tree(&mut mapping);
    let mut targets = HashMap::new();

    let mut should_quit = false;
    while !should_quit {
        terminal.draw(|frame| {
            // Recalculate layout if the frame has changed sizing
            if last_computed_size != Some(frame.size()) {
                let viewport_size = Size {
                    width: AvailableSpace::Definite(frame.size().width as f32),
                    height: AvailableSpace::Definite(frame.size().height as f32),
                };
                tree.compute_layout(root_node, viewport_size).unwrap();

                last_computed_size = Some(frame.size());
            }

            targets.clear();
            // Set the PRNG seed so each render uses the same (random) color for each block
            create_layout(&mut tree, root_node, frame, &mapping, &mut targets);
        })?;
        should_quit = handle_events(&targets)?;
    }

    disable_raw_mode()?;
    stdout().execute(DisableMouseCapture)?.execute(LeaveAlternateScreen)?;

    Ok(())
}

fn generate_taffy_tree(mapping: &mut HashMap<NodeId, ConfigEntry>) -> (TaffyTree, NodeId) {
    let mut tree: TaffyTree<()> = TaffyTree::new();

    let mut children = Vec::new();
    for c in CONFIG {
        let id = tree.new_leaf(taffy::Style {
            size: Size { width: Dimension::Auto, height: Dimension::Length(5.0) },
            display: Display::Block,
            ..Default::default()
        }).unwrap();

        mapping.insert(id, *c);
        children.push(id);
    }

    // Root node
    let root_node = tree.new_with_children(
        taffy::Style {
            size: Size { width: Dimension::Percent(1.0), height: Dimension::Percent(1.0) },
            grid_template_columns: vec![TrackSizingFunction::Repeat(AutoFit, vec![MinMax {
                min: MinTrackSizingFunction::Fixed(LengthPercentage::Length(10.0)),
                max: MaxTrackSizingFunction::Fixed(LengthPercentage::Length(40.0)),
            }])],
            display: Display::Grid,
            ..Default::default()
        },
        &children,
    ).unwrap();

    (tree, root_node)
}

fn create_layout(
    tree: &TaffyTree,
    node_id: NodeId,
    frame: &mut Frame,
    mapping: &HashMap<NodeId, ConfigEntry>,
    targets: &mut HashMap<Rect, &'static [u8]>,
) {
    let layout = tree.get_final_layout(node_id);

    let r = Rect::new(
        layout.location.x as u16,
        layout.location.y as u16,
        layout.size.width as u16,
        layout.size.height as u16,
    );

    if let Some((key, (name, data))) = mapping.get(&node_id) {
        let title = if let KeyCode::Char(c) = key {
            format!("[{c}]")
        } else {
            format!("{:?}", key)
        };

        let b = Block::new()
            .title(title)
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::White))
            .padding(Padding::new(
                0, // left
                0, // right
                r.height / 3, // top
                0, // bottom
            ));

        targets.insert(r, data);

        let p = Paragraph::new(Span::raw(name.to_string()));
        frame.render_widget(p.block(b).alignment(Alignment::Center), r);
    }

    for child_node_id in tree.child_ids(node_id) {
        create_layout(tree, child_node_id, frame, mapping, targets);
    }
}

fn handle_events(targets: &HashMap<Rect, &'static [u8]>) -> color_eyre::Result<bool> {
    if event::poll(std::time::Duration::from_millis(50))? {
        match event::read()? {
            Event::Key(key) => {
                if key.kind == event::KeyEventKind::Press {
                    for (ckey, (_, data)) in CONFIG {
                        if &key.code == ckey {
                            thread::spawn(|| if let Err(e) = play_sound(data) {
                                println!("{:?}", e);
                            });
                        }
                    }

                    if key.code == KeyCode::Esc {
                        return Ok(true);
                    }
                }
            }
            Event::Mouse(m) => {
                if let MouseEventKind::Up(_) = m.kind {
                    for (r, d) in targets {
                        if r.contains(Position::new(m.column, m.row)) {
                            thread::spawn(|| if let Err(e) = play_sound(d) {
                                println!("{:?}", e);
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(false)
}

fn play_sound(data: &[u8]) -> color_eyre::Result<()> {
    // Get a output stream handle to the default physical sound device
    let (_stream, stream_handle) = OutputStream::try_default().wrap_err("stream")?;
    // Decode that sound file into a source
    let source = Decoder::new(Cursor::new(data.to_vec())).wrap_err("decoder")?;

    let sink = Sink::try_new(&stream_handle).wrap_err("get sink")?;
    sink.append(source);
    sink.sleep_until_end();

    Ok(())
}
