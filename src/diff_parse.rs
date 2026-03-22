use color_eyre::eyre::{Context, Result};
use unidiff::PatchSet;

use crate::pull_changes::PullRequest;

/// Parses a unified diff string
fn parse_diff(input: &str) -> Result<PatchSet> {
    // Note: this wrapper also allows us to contain and limit mutation
    let mut patch = PatchSet::new();
    patch.parse(input).wrap_err("could not parse git diff")?;
    Ok(patch)
}

impl PullRequest {
    /// Parse the diff string into a structured set of changes
    pub(crate) fn parse_diff(&self) -> Result<PatchSet> {
        let diff = self
            .fetch_diff()
            .wrap_err("could not fetch diff while generating parsed diff")?;

        parse_diff(&diff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unidiff::{LINE_TYPE_ADDED, LINE_TYPE_CONTEXT, LINE_TYPE_REMOVED};

    // ========================================================================
    // Basic parsing
    // ========================================================================

    #[test]
    fn parse_empty_input() {
        let patch = parse_diff("").unwrap();
        assert_eq!(patch.len(), 0);
    }

    #[test]
    fn parse_single_file_single_hunk() {
        let input = "\
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,5 +1,6 @@
 fn main() {
-    println!(\"hello\");
+    println!(\"hello world\");
+    println!(\"goodbye\");
     let x = 1;
     let y = 2;
 }
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch.len(), 1);

        let file = &patch[0];
        assert_eq!(file.path(), "src/main.rs");
        assert_eq!(file.hunks().len(), 1);

        let hunk = &file.hunks()[0];
        assert_eq!(hunk.source_start, 1);
        assert_eq!(hunk.source_length, 5);
        assert_eq!(hunk.target_start, 1);
        assert_eq!(hunk.target_length, 6);
    }

    // ========================================================================
    // Line types
    // ========================================================================

    #[test]
    fn line_types_are_correct() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,3 @@
 context
-removed
+added
";
        let patch = parse_diff(input).unwrap();
        let lines = patch[0].hunks()[0].lines();

        assert!(lines[0].is_context());
        assert_eq!(lines[0].line_type, LINE_TYPE_CONTEXT);

        assert!(lines[1].is_removed());
        assert_eq!(lines[1].line_type, LINE_TYPE_REMOVED);

        assert!(lines[2].is_added());
        assert_eq!(lines[2].line_type, LINE_TYPE_ADDED);
    }

    #[test]
    fn line_values_are_correct() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,3 @@
 fn example() {
-    let result = a + b;
+    let result = a + b + c;
";
        let patch = parse_diff(input).unwrap();
        let lines = patch[0].hunks()[0].lines();

        assert_eq!(lines[0].value, "fn example() {");
        assert_eq!(lines[1].value, "    let result = a + b;");
        assert_eq!(lines[2].value, "    let result = a + b + c;");
    }

    // ========================================================================
    // Line numbers
    // ========================================================================

    #[test]
    fn context_line_has_both_line_numbers() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,3 @@
 context
-old
+new
";
        let patch = parse_diff(input).unwrap();
        let line = &patch[0].hunks()[0].lines()[0];

        assert_eq!(line.source_line_no, Some(1));
        assert_eq!(line.target_line_no, Some(1));
    }

    #[test]
    fn removed_line_has_only_source_line_number() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,2 @@
 context
-removed
 end
";
        let patch = parse_diff(input).unwrap();
        let line = &patch[0].hunks()[0].lines()[1];

        assert!(line.is_removed());
        assert_eq!(line.source_line_no, Some(2));
        assert_eq!(line.target_line_no, None);
    }

    #[test]
    fn added_line_has_only_target_line_number() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -1,2 +1,3 @@
 context
+added
 end
";
        let patch = parse_diff(input).unwrap();
        let line = &patch[0].hunks()[0].lines()[1];

        assert!(line.is_added());
        assert_eq!(line.source_line_no, None);
        assert_eq!(line.target_line_no, Some(2));
    }

    #[test]
    fn line_numbers_track_through_additions() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,5 @@
 first
+new_a
+new_b
 second
 third
";
        let patch = parse_diff(input).unwrap();
        let lines = patch[0].hunks()[0].lines();

        // first: old=1, new=1
        assert_eq!(lines[0].source_line_no, Some(1));
        assert_eq!(lines[0].target_line_no, Some(1));

        // new_a: old=None, new=2
        assert_eq!(lines[1].source_line_no, None);
        assert_eq!(lines[1].target_line_no, Some(2));

        // new_b: old=None, new=3
        assert_eq!(lines[2].source_line_no, None);
        assert_eq!(lines[2].target_line_no, Some(3));

        // second: old=2, new=4
        assert_eq!(lines[3].source_line_no, Some(2));
        assert_eq!(lines[3].target_line_no, Some(4));

        // third: old=3, new=5
        assert_eq!(lines[4].source_line_no, Some(3));
        assert_eq!(lines[4].target_line_no, Some(5));
    }

    #[test]
    fn line_numbers_track_through_deletions() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -1,4 +1,2 @@
 first
-removed_a
-removed_b
 last
";
        let patch = parse_diff(input).unwrap();
        let lines = patch[0].hunks()[0].lines();

        assert_eq!(lines[0].source_line_no, Some(1));
        assert_eq!(lines[0].target_line_no, Some(1));

        assert_eq!(lines[1].source_line_no, Some(2));
        assert_eq!(lines[1].target_line_no, None);

        assert_eq!(lines[2].source_line_no, Some(3));
        assert_eq!(lines[2].target_line_no, None);

        // last: old=4, new=2
        assert_eq!(lines[3].source_line_no, Some(4));
        assert_eq!(lines[3].target_line_no, Some(2));
    }

    #[test]
    fn line_numbers_with_nonzero_hunk_start() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -50,3 +70,4 @@
 context
-removed
+added_1
+added_2
";
        let patch = parse_diff(input).unwrap();
        let lines = patch[0].hunks()[0].lines();

        assert_eq!(lines[0].source_line_no, Some(50));
        assert_eq!(lines[0].target_line_no, Some(70));

        assert_eq!(lines[1].source_line_no, Some(51));
        assert_eq!(lines[1].target_line_no, None);

        assert_eq!(lines[2].source_line_no, None);
        assert_eq!(lines[2].target_line_no, Some(71));

        assert_eq!(lines[3].source_line_no, None);
        assert_eq!(lines[3].target_line_no, Some(72));
    }

    // ========================================================================
    // Multiple hunks
    // ========================================================================

    #[test]
    fn multiple_hunks_in_one_file() {
        let input = "\
--- a/lib.rs
+++ b/lib.rs
@@ -1,3 +1,3 @@
 fn a() {
-    old_a
+    new_a
 }
@@ -20,3 +20,4 @@
 fn b() {
     keep
+    added
 }
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch[0].hunks().len(), 2);

        let h0 = &patch[0].hunks()[0];
        assert_eq!(h0.source_start, 1);
        assert_eq!(h0.source_length, 3);

        let h1 = &patch[0].hunks()[1];
        assert_eq!(h1.source_start, 20);
        assert_eq!(h1.target_length, 4);
    }

    #[test]
    fn hunk_line_numbers_are_independent() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -1,2 +1,2 @@
-old_first
+new_first
 context_a
@@ -100,2 +100,2 @@
-old_hundred
+new_hundred
 context_b
";
        let patch = parse_diff(input).unwrap();
        let hunks = patch[0].hunks();

        assert_eq!(hunks[0].lines()[0].source_line_no, Some(1));
        assert_eq!(hunks[1].lines()[0].source_line_no, Some(100));
    }

    // ========================================================================
    // Multiple files
    // ========================================================================

    #[test]
    fn multiple_files() {
        let input = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,3 @@
 fn foo() {
-    1
+    2
 }
--- a/bar.rs
+++ b/bar.rs
@@ -1,3 +1,4 @@
 fn bar() {
     3
+    4
 }
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch.len(), 2);
        assert_eq!(patch[0].path(), "foo.rs");
        assert_eq!(patch[1].path(), "bar.rs");
    }

    #[test]
    fn three_files() {
        let input = "\
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
-old_a
+new_a
--- a/b.rs
+++ b/b.rs
@@ -1,2 +1,2 @@
-old_b
+new_b
--- a/c.rs
+++ b/c.rs
@@ -1,2 +1,2 @@
-old_c
+new_c
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch.len(), 3);
        assert_eq!(patch[0].path(), "a.rs");
        assert_eq!(patch[1].path(), "b.rs");
        assert_eq!(patch[2].path(), "c.rs");
    }

    // ========================================================================
    // New / deleted files
    // ========================================================================

    #[test]
    fn new_file() {
        let input = "\
--- /dev/null
+++ b/brand_new.rs
@@ -0,0 +1,3 @@
+fn hello() {
+    println!(\"hi\");
+}
";
        let patch = parse_diff(input).unwrap();
        let file = &patch[0];
        assert_eq!(file.source_file, "/dev/null");
        assert_eq!(file.path(), "brand_new.rs");
        assert!(file.is_added_file());
        assert!(file.hunks()[0].lines().iter().all(|l| l.is_added()));
    }

    #[test]
    fn deleted_file() {
        let input = "\
--- a/gone.rs
+++ /dev/null
@@ -1,3 +0,0 @@
-fn goodbye() {
-    println!(\"bye\");
-}
";
        let patch = parse_diff(input).unwrap();
        let file = &patch[0];
        assert_eq!(file.path(), "gone.rs");
        assert!(file.is_removed_file());
        assert!(file.hunks()[0].lines().iter().all(|l| l.is_removed()));
    }

    // ========================================================================
    // Extended headers (git diff format)
    // ========================================================================

    #[test]
    fn diff_with_git_headers() {
        let input = "\
diff --git a/src/main.rs b/src/main.rs
index abc1234..def5678 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,3 @@
 fn main() {
-    old();
+    new();
 }
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch.len(), 1);
        assert_eq!(patch[0].path(), "src/main.rs");
        assert_eq!(patch[0].hunks().len(), 1);
    }

    #[test]
    fn diff_with_new_file_mode_header() {
        let input = "\
diff --git a/new.rs b/new.rs
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/new.rs
@@ -0,0 +1,2 @@
+fn new() {
+}
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch.len(), 1);
        assert!(patch[0].is_added_file());
    }

    #[test]
    fn diff_with_rename_headers() {
        let input = "\
diff --git a/old_name.rs b/new_name.rs
similarity index 95%
rename from old_name.rs
rename to new_name.rs
index abc123..def456 100644
--- a/old_name.rs
+++ b/new_name.rs
@@ -1,3 +1,3 @@
 fn foo() {
-    old
+    new
 }
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch.len(), 1);
        assert_eq!(patch[0].source_file, "a/old_name.rs");
        assert_eq!(patch[0].target_file, "b/new_name.rs");
    }

    // ========================================================================
    // Mixed scenarios
    // ========================================================================

    #[test]
    fn mixed_new_modified_deleted() {
        let input = "\
diff --git a/modified.rs b/modified.rs
--- a/modified.rs
+++ b/modified.rs
@@ -1,3 +1,3 @@
 fn a() {
-    old
+    new
 }
diff --git a/created.rs b/created.rs
--- /dev/null
+++ b/created.rs
@@ -0,0 +1,2 @@
+fn b() {
+}
diff --git a/deleted.rs b/deleted.rs
--- a/deleted.rs
+++ /dev/null
@@ -1,2 +0,0 @@
-fn c() {
-}
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch.len(), 3);

        assert!(patch[0].is_modified_file());
        assert!(patch[1].is_added_file());
        assert!(patch[2].is_removed_file());
    }

    #[test]
    fn file_with_many_hunks() {
        let input = "\
diff --git a/big.rs b/big.rs
--- a/big.rs
+++ b/big.rs
@@ -1,3 +1,3 @@
 fn a() {
-    old_a
+    new_a
 }
@@ -50,3 +50,3 @@
 fn b() {
-    old_b
+    new_b
 }
@@ -100,3 +100,4 @@
 fn c() {
     keep
+    added
 }
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch.len(), 1);
        assert_eq!(patch[0].hunks().len(), 3);
        assert_eq!(patch[0].hunks()[0].source_start, 1);
        assert_eq!(patch[0].hunks()[1].source_start, 50);
        assert_eq!(patch[0].hunks()[2].source_start, 100);
    }

    // ========================================================================
    // Hunk metadata
    // ========================================================================

    #[test]
    fn hunk_section_header() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -10,3 +10,3 @@ fn some_function()
 context
-old
+new
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch[0].hunks()[0].section_header, "fn some_function()");
    }

    #[test]
    fn hunk_added_removed_counts() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -1,4 +1,5 @@
 context
-removed_1
-removed_2
+added_1
+added_2
+added_3
 end
";
        let patch = parse_diff(input).unwrap();
        let hunk = &patch[0].hunks()[0];
        assert_eq!(hunk.added(), 3);
        assert_eq!(hunk.removed(), 2);
    }

    // ========================================================================
    // PatchedFile metadata
    // ========================================================================

    #[test]
    fn file_added_removed_counts() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,3 @@
 context
-old
+new
@@ -10,2 +10,3 @@
 context
+added
";
        let patch = parse_diff(input).unwrap();
        let file = &patch[0];
        assert_eq!(file.added(), 2);
        assert_eq!(file.removed(), 1);
    }

    #[test]
    fn file_path_strips_prefix() {
        let input = "\
--- a/deeply/nested/path/file.rs
+++ b/deeply/nested/path/file.rs
@@ -1,2 +1,2 @@
-old
+new
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch[0].path(), "deeply/nested/path/file.rs");
    }

    // ========================================================================
    // Edge cases
    // ========================================================================

    #[test]
    fn single_line_change() {
        let input = "\
--- a/one.rs
+++ b/one.rs
@@ -1 +1 @@
-old_single_line
+new_single_line
";
        let patch = parse_diff(input).unwrap();
        let hunk = &patch[0].hunks()[0];
        assert_eq!(hunk.source_start, 1);
        assert_eq!(hunk.source_length, 1);
        assert_eq!(hunk.target_start, 1);
        assert_eq!(hunk.target_length, 1);
        assert_eq!(hunk.lines().len(), 2);
    }

    #[test]
    fn deeply_indented_code() {
        let input = "\
--- a/file.rs
+++ b/file.rs
@@ -1,3 +1,3 @@
         fn deeply() {
-            old_deep();
+            new_deep();
         }
";
        let patch = parse_diff(input).unwrap();
        let lines = patch[0].hunks()[0].lines();
        assert_eq!(lines[0].value, "        fn deeply() {");
        assert_eq!(lines[1].value, "            old_deep();");
        assert_eq!(lines[2].value, "            new_deep();");
    }

    #[test]
    fn content_with_plus_and_minus_characters() {
        let input = "\
--- a/math.rs
+++ b/math.rs
@@ -1,3 +1,3 @@
 let x = a + b;
-let y = c - d;
+let y = c - d + e;
";
        let patch = parse_diff(input).unwrap();
        let lines = patch[0].hunks()[0].lines();
        assert_eq!(lines[0].value, "let x = a + b;");
        assert_eq!(lines[1].value, "let y = c - d;");
        assert_eq!(lines[2].value, "let y = c - d + e;");
    }

    #[test]
    fn large_line_numbers() {
        let input = "\
--- a/big.rs
+++ b/big.rs
@@ -1024,3 +2048,3 @@
 context
-old_at_1025
+new_at_2049
";
        let patch = parse_diff(input).unwrap();
        let lines = patch[0].hunks()[0].lines();
        assert_eq!(lines[0].source_line_no, Some(1024));
        assert_eq!(lines[0].target_line_no, Some(2048));
        assert_eq!(lines[1].source_line_no, Some(1025));
        assert_eq!(lines[2].target_line_no, Some(2049));
    }

    #[test]
    fn diff_with_multiple_extended_headers() {
        let input = "\
diff --git a/a.rs b/a.rs
index 1234567..abcdefg 100644
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
-old
+new
diff --git a/b.rs b/b.rs
new file mode 100644
index 0000000..1234567
--- /dev/null
+++ b/b.rs
@@ -0,0 +1,1 @@
+fresh
";
        let patch = parse_diff(input).unwrap();
        assert_eq!(patch.len(), 2);
        assert_eq!(patch[0].path(), "a.rs");
        assert_eq!(patch[1].path(), "b.rs");
    }

    #[test]
    fn line_numbers_across_multiple_hunks() {
        let input = "\
--- a/multi.rs
+++ b/multi.rs
@@ -1,2 +1,3 @@
 line_one
+inserted
 line_two
@@ -10,2 +11,2 @@
-old_ten
+new_eleven
 line_eleven
";
        let patch = parse_diff(input).unwrap();
        let hunks = patch[0].hunks();

        assert_eq!(hunks[0].lines()[0].source_line_no, Some(1));
        assert_eq!(hunks[0].lines()[0].target_line_no, Some(1));
        assert_eq!(hunks[0].lines()[1].target_line_no, Some(2)); // inserted
        assert_eq!(hunks[0].lines()[2].source_line_no, Some(2));
        assert_eq!(hunks[0].lines()[2].target_line_no, Some(3));

        assert_eq!(hunks[1].lines()[0].source_line_no, Some(10));
        assert_eq!(hunks[1].lines()[0].target_line_no, None);
        assert_eq!(hunks[1].lines()[1].source_line_no, None);
        assert_eq!(hunks[1].lines()[1].target_line_no, Some(11));
    }
}
