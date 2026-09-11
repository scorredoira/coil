use helix_core::hashmap;
use helix_term::keymap;
use helix_view::document::Mode;

use super::*;

fn with_shortcuts() -> AppBuilder {
    let mut config = helpers::test_config();
    config.keys.insert(
        Mode::Normal,
        keymap!({"Normal mode"
            "C-g" => goto_line_prompt,
            "C-f" => search_in_file,
            "backspace" => delete_selection_or_previous_char,
        }),
    );

    AppBuilder::new().with_config(config)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cmd_key_nobody_bound_writes_nothing() -> anyhow::Result<()> {
    test(("#[a|]#", "i<Cmd-c>x<C-b>y<esc>", "xy#[|a]#")).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn go_to_line_asks_for_the_number() -> anyhow::Result<()> {
    test_with_config(
        with_shortcuts(),
        (
            "#[o|]#ne\ntwo\nthree\nfour\n",
            "<C-g>3<ret>",
            "one\ntwo\n#[t|]#hree\nfour\n",
        ),
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn go_to_line_puts_the_cursor_back_on_escape() -> anyhow::Result<()> {
    test_with_config(
        with_shortcuts(),
        (
            "one\n#[t|]#wo\nthree\nfour\n",
            "<C-g>4<esc>",
            "one\n#[t|]#wo\nthree\nfour\n",
        ),
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn go_to_line_refuses_what_is_not_a_number() -> anyhow::Result<()> {
    test_with_config(
        with_shortcuts(),
        (
            "one\n#[t|]#wo\nthree\n",
            "<C-g>3x<ret>",
            "one\n#[t|]#wo\nthree\n",
        ),
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn search_in_file_replaces_in_this_buffer() -> anyhow::Result<()> {
    test_with_config(
        with_shortcuts(),
        (
            "#[l|]#et tenant = 1;\nlet other = tenant;\n",
            "<C-f>tenant<A-h><tab>account<A-a><esc>",
            "#[l|]#et account = 1;\nlet other = account;\n",
        ),
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn backspace_takes_the_character_before_a_bare_cursor() -> anyhow::Result<()> {
    test_with_config(
        with_shortcuts(),
        ("hello #[w|]#orld", "<backspace>", "hello#[w|]#orld"),
    )
    .await?;

    // At the very start there is nothing before it.
    test_with_config(
        with_shortcuts(),
        ("#[h|]#ello", "<backspace>", "#[h|]#ello"),
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn backspace_deletes_what_is_selected() -> anyhow::Result<()> {
    test_with_config(
        with_shortcuts(),
        ("#[hello|]# world", "<backspace>", "#[ |]#world"),
    )
    .await?;

    // A selection and a cursor right after it: the cursor does not reach into it.
    test_with_config(
        with_shortcuts(),
        ("#[ab|]##(c|)#d", "<backspace>", "#[c|]#d"),
    )
    .await?;

    Ok(())
}
