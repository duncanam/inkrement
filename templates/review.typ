// inkrement review template
// Designed for reMarkable Paper Pro (11.8" e-ink display)
// Toolbar can be on any edge, so all margins are generous

#set page(
  width: 8.3in,
  height: 11.1in,
  margin: 0.7in,
)

#set text(font: "Inter", size: 9pt)
#show raw: set text(font: "JetBrains Mono", size: 8pt)
#set par(leading: 0.5em)

// Make headings invisible - they only exist for PDF bookmark navigation
#show heading: it => none

// Load review data from JSON (served by the Rust World impl)
#let data = json("review-data.json")

// Colors
#let red = rgb("#cc0000")
#let green = rgb("#228b22")
#let gray = rgb("#888888")
#let light-gray = rgb("#dddddd")
#let faint-gray = rgb("#f5f5f5")
#let added-bg = rgb("#e6ffe6")
#let removed-bg = rgb("#ffe6e6")

// Shared code block renderer.
// - `code-text`: the joined source string
// - `lang`: language for syntax highlighting
// - `line-nos`: array of line numbers (int or none) parallel to raw lines
// - `kinds`: array of "added"/"removed"/"context" parallel to raw lines (or none for plain)
// - `bar-color`: color for the margin bar on changed lines (or none)
#let code-block(code-text, lang, line-nos, kinds: none, bar-color: none) = {
  let effective-lang = if lang != "" { lang } else { "txt" }

  block(
    width: 100%,
    inset: 6pt,
    stroke: 0.5pt + light-gray,
    radius: 2pt,
  )[
    #set par(leading: 0.4em)
    #show raw.line: it => {
      let idx = it.number - 1
      let no = line-nos.at(idx, default: none)
      let kind = if kinds != none { kinds.at(idx, default: "context") } else { "context" }
      let bg = if kind == "added" { added-bg } else if kind == "removed" { removed-bg } else { none }
      let bar = if kind != "context" and bar-color != none {
        box(width: 3pt, height: 8pt, fill: bar-color)
        h(4pt)
      } else {
        h(if bar-color != none { 7pt } else { 0pt })
      }

      box(
        width: 100%,
        fill: bg,
        inset: (x: 4pt, y: 2pt),
      )[
        #grid(
          columns: if bar-color != none { (24pt, auto, 1fr) } else { (28pt, 1fr) },
          align: horizon,
          text(size: 7pt, fill: gray)[
            #if no != none [#str(no)]
          ],
          ..if bar-color != none { (bar,) },
          it,
        )
      ]
    }
    #raw(code-text, lang: effective-lang, block: true)
  ]
}

// Diff code block: filters lines by kind, extracts metadata, delegates to code-block
#let diff-code-block(lines, lang, side, filter, bar-color) = {
  let filtered = lines.filter(l => l.kind in filter)
  if filtered.len() == 0 { return }
  let code-text = filtered.map(l => l.content).join("\n")
  let kinds = filtered.map(l => l.kind)
  let line-nos = filtered.map(l => {
    if side == "old" { l.old_line_no } else { l.new_line_no }
  })
  code-block(code-text, lang, line-nos, kinds: kinds, bar-color: bar-color)
}

// Source code block: plain display with line numbers, no diff coloring
#let source-code-block(source, lang) = {
  // Build line numbers 1..N
  let n = source.split("\n").len()
  let line-nos = range(1, n + 1)
  code-block(source, lang, line-nos)
}

// ============================================================================
// Title page
// ============================================================================
#page[
  == Review
  #v(0.8in)

  // PR info box
  #block(
    width: 100%,
    inset: 16pt,
    radius: 6pt,
    fill: faint-gray,
    stroke: 0.5pt + light-gray,
  )[
    #align(center)[
      #text(size: 22pt, weight: "bold", tracking: -0.5pt)[#data.title]
    ]

    #v(0.3in)

    #let is-incremental = data.diff_mode == "incremental"
    #if is-incremental [
      #box(
        inset: (x: 8pt, y: 4pt),
        radius: 3pt,
        fill: rgb("#e6f0ff"),
        stroke: 0.5pt + rgb("#4488cc"),
      )[#text(size: 9pt, weight: "bold", fill: rgb("#4488cc"))[Incremental Review]]
      #v(10pt)
    ]

    #grid(
      columns: (1in, 1fr),
      row-gutter: 10pt,
      text(size: 9pt, fill: gray)[Pull Request], text(size: 9pt, weight: "bold")[#data.repo \##str(data.number)],
      text(size: 9pt, fill: gray)[Author], text(size: 9pt, weight: "bold")[#data.author],
      text(size: 9pt, fill: gray)[Reviewer], text(size: 9pt, weight: "bold")[#data.reviewer],
      text(size: 9pt, fill: gray)[#if is-incremental [Since] else [Base]], raw(data.diff_base),
      text(size: 9pt, fill: gray)[Head], raw(data.head_ref),
      text(size: 9pt, fill: gray)[Added], text(size: 9pt, fill: green, weight: "bold")[+#str(data.lines_added) lines],
      text(size: 9pt, fill: gray)[Removed], text(size: 9pt, fill: red, weight: "bold")[-#str(data.lines_removed) lines],
    )
  ]

  #v(0.4in)

  #block(
    width: 100%,
    inset: 16pt,
    radius: 6pt,
    fill: faint-gray,
    stroke: 0.5pt + light-gray,
  )[
    #text(size: 13pt, weight: "bold")[Review Decision]
    #v(10pt)
    #grid(
      columns: (20pt, 1fr),
      row-gutter: 14pt,
      column-gutter: 10pt,
      align: horizon,
      rect(width: 16pt, height: 16pt, stroke: 1pt, fill: white, radius: 2pt),
      text(size: 11pt, fill: green, weight: "bold")[Approve],
      rect(width: 16pt, height: 16pt, stroke: 1pt, fill: white, radius: 2pt),
      text(size: 11pt, fill: red, weight: "bold")[Request Changes],
      rect(width: 16pt, height: 16pt, stroke: 1pt, fill: white, radius: 2pt),
      text(size: 11pt)[Comment Only],
    )
  ]
]

// ============================================================================
// Page 2: Files changed
// ============================================================================
== Files Changed
#page[
  #text(size: 16pt, weight: "bold")[Files Changed]
  #v(0.1in)
  #line(length: 100%, stroke: 0.5pt + light-gray)
  #v(0.15in)

  #for (i, file) in data.files.enumerate() [
    #link(label("file-" + str(i) + "-0"))[
      #block(
        width: 100%,
        inset: (x: 10pt, y: 8pt),
        radius: 4pt,
        fill: faint-gray,
        stroke: 0.5pt + light-gray,
      )[
        #text(size: 10pt, weight: "bold")[#file.path]
        #h(1fr)
        #text(size: 8pt, fill: green, weight: "bold")[+#str(file.added)]
        #h(4pt)
        #text(size: 8pt, fill: red, weight: "bold")[-#str(file.removed)]
      ]
    ]
    #v(4pt)
  ]
]

// ============================================================================
// Diff pages - one page per hunk
// ============================================================================
#for (file-idx, file) in data.files.enumerate() [
  #for (hunk-idx, hunk) in file.hunks.enumerate() [
    #page[
      #if hunk-idx == 0 [= #file.path]
      #text(size: 10pt, weight: "bold")[#file.path] #label("file-" + str(file-idx) + "-" + str(hunk-idx))
      #h(1fr)
      #text(size: 9pt, fill: gray)[\[H#str(hunk.id)\]]
      #line(length: 100%, stroke: 0.5pt + light-gray)
      #v(6pt)

      // Removed
      #{
        let has-old = hunk.lines.filter(l => l.kind in ("removed", "context")).len() > 0
        if has-old [
          #box(
            inset: (x: 8pt, y: 4pt),
            radius: 3pt,
            fill: removed-bg,
            stroke: 0.5pt + red,
          )[#text(size: 10pt, weight: "bold", fill: red)[Removed]]
          #if file.old_source_label != none [
            #h(1fr)
            #link(label(file.old_source_label))[
              #text(size: 8pt, fill: gray)[\[full source\]]
            ]
          ]
          #v(4pt)
          #diff-code-block(hunk.lines, file.lang, "old", ("removed", "context"), red)
          #v(12pt)
        ]
      }

      // Added
      #{
        let has-new = hunk.lines.filter(l => l.kind in ("added", "context")).len() > 0
        if has-new [
          #box(
            inset: (x: 8pt, y: 4pt),
            radius: 3pt,
            fill: added-bg,
            stroke: 0.5pt + green,
          )[#text(size: 10pt, weight: "bold", fill: green)[Added]]
          #if file.new_source_label != none [
            #h(1fr)
            #link(label(file.new_source_label))[
              #text(size: 8pt, fill: gray)[\[full source\]]
            ]
          ]
          #v(4pt)
          #diff-code-block(hunk.lines, file.lang, "new", ("added", "context"), green)
        ]
      }

      #v(1fr)
    ]
  ]
]

// ============================================================================
// Separator page - marks the end of reviewable content
// ============================================================================
#page[
  = Reference Source Code
  #v(1fr)
  #align(center)[
    #text(size: 24pt, weight: "bold", fill: gray)[REFERENCE SOURCE CODE]
    #v(0.2in)
    #text(size: 11pt, fill: gray)[
      The following pages contain full source files for reference only. \
      No annotations on these pages will be processed.
    ]
  ]
  #v(1fr)
]

// ============================================================================
// Full source pages - old versions
// ============================================================================
#for (file-idx, file) in data.files.enumerate() [
  #if file.old_source != none [
    #page[
      #text(size: 9pt, weight: "bold")[
        #file.path #text(fill: gray)[(old)]
      ] #label("source-old-" + str(file-idx))
      #line(length: 100%, stroke: 0.5pt + light-gray)
      #v(4pt)
      #source-code-block(file.old_source, file.lang)
    ]
  ]
]

// ============================================================================
// Full source pages - new versions
// ============================================================================
#for (file-idx, file) in data.files.enumerate() [
  #if file.new_source != none [
    #page[
      #text(size: 9pt, weight: "bold")[
        #file.path #text(fill: gray)[(new)]
      ] #label("source-new-" + str(file-idx))
      #line(length: 100%, stroke: 0.5pt + light-gray)
      #v(4pt)
      #source-code-block(file.new_source, file.lang)
    ]
  ]
]
