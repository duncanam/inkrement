// inkrement review template
// Designed for reMarkable Paper Pro (11.8" e-ink display)
// Toolbar can be on any edge, so all margins are generous

#set page(
  width: 8.3in,
  height: 11.1in,
  margin: 1in,
)

#set text(font: "Inter", size: 9pt)
#show raw: set text(font: "JetBrains Mono", size: 8pt)
#set par(leading: 0.5em)

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

// Helper: render a diff code block with syntax highlighting, line numbers, and bars
#let diff-code-block(lines, lang, side, filter, bar-color) = {
  let filtered = lines.filter(l => l.kind in filter)
  if filtered.len() == 0 { return }
  let code-text = filtered.map(l => l.content).join("\n")
  let kinds = filtered.map(l => l.kind)
  let line-nos = filtered.map(l => {
    if side == "old" { l.old_line_no } else { l.new_line_no }
  })
  let effective-lang = if lang != "" { lang } else { "txt" }

  block(
    width: 100%,
    inset: 6pt,
    stroke: 0.5pt + light-gray,
    radius: 2pt,
  )[
    #show raw.line: it => {
      let idx = it.number - 1
      let kind = kinds.at(idx, default: "context")
      let no = line-nos.at(idx, default: none)
      let bg = if kind == "added" { added-bg } else if kind == "removed" { removed-bg } else { none }
      box(
        width: 100%,
        fill: bg,
        inset: (x: 2pt, y: 0.5pt),
      )[
        #grid(
          columns: (24pt, auto, 1fr),
          align: horizon,
          text(size: 7pt, fill: gray)[
            #if no != none [#str(no)]
          ],
          if kind != "context" [
            #box(width: 3pt, height: 8pt, fill: bar-color)
            #h(4pt)
          ] else [
            #h(7pt)
          ],
          it,
        )
      ]
    }
    #raw(code-text, lang: effective-lang, block: true)
  ]
}

// ============================================================================
// Title page
// ============================================================================
#page[
  #v(1.2in)

  #align(center)[
    #text(size: 22pt, weight: "bold", tracking: -0.5pt)[#data.title]
    #v(0.15in)
    #box(
      inset: (x: 10pt, y: 5pt),
      radius: 4pt,
      fill: faint-gray,
      stroke: 0.5pt + light-gray,
    )[
      #text(size: 11pt, fill: gray)[#data.repo]
      #h(4pt)
      #text(size: 11pt, weight: "bold")[\##str(data.number)]
    ]
  ]

  #v(0.5in)

  #pad(x: 0.5in)[
    #grid(
      columns: (1in, 1fr),
      row-gutter: 10pt,
      text(size: 9pt, fill: gray)[Author], text(size: 9pt, weight: "bold")[#data.author],
      text(size: 9pt, fill: gray)[Reviewer], text(size: 9pt, weight: "bold")[#data.reviewer],
      text(size: 9pt, fill: gray)[Base], raw(data.base_ref),
      text(size: 9pt, fill: gray)[Head], raw(data.head_ref),
      text(size: 9pt, fill: gray)[Added], text(size: 9pt, fill: green, weight: "bold")[+#str(data.lines_added) lines],
      text(size: 9pt, fill: gray)[Removed], text(size: 9pt, fill: red, weight: "bold")[-#str(data.lines_removed) lines],
    )
  ]

  #v(0.5in)

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
#page[
  #text(size: 16pt, weight: "bold")[Files Changed]
  #v(0.1in)
  #line(length: 100%, stroke: 0.5pt + light-gray)
  #v(0.15in)

  #for (i, file) in data.files.enumerate() [
    #link(label("file-" + str(i) + "-0"))[#text(size: 10pt)[#file.path]] \
    #v(2pt)
  ]
]

// ============================================================================
// Diff pages — one page per hunk
// ============================================================================
#for (file-idx, file) in data.files.enumerate() [
  #for (hunk-idx, hunk) in file.hunks.enumerate() [
    #page[
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
// Full source pages — old versions
// ============================================================================
#for (file-idx, file) in data.files.enumerate() [
  #if file.old_source != none [
    #page[
      #text(size: 9pt, weight: "bold")[
        #file.path #text(fill: gray)[(old)]
      ] #label("source-old-" + str(file-idx))
      #line(length: 100%, stroke: 0.5pt + light-gray)
      #v(4pt)
      #raw(file.old_source, lang: if file.lang != "" { file.lang } else { "txt" }, block: true)
    ]
  ]
]

// ============================================================================
// Full source pages — new versions
// ============================================================================
#for (file-idx, file) in data.files.enumerate() [
  #if file.new_source != none [
    #page[
      #text(size: 9pt, weight: "bold")[
        #file.path #text(fill: gray)[(new)]
      ] #label("source-new-" + str(file-idx))
      #line(length: 100%, stroke: 0.5pt + light-gray)
      #v(4pt)
      #raw(file.new_source, lang: if file.lang != "" { file.lang } else { "txt" }, block: true)
    ]
  ]
]
