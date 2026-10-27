# Contributing to lazyrunner

Pull requests are welcome. lazyrunner is source-available under the
[lazyrunner License](LICENSE), not open source. Contributions come in on the
terms below, so the project keeps one owner and one official repository.

## How to contribute

1. For anything larger than a small fix, open an issue first, so we can agree
   on the change before you spend time on it.
2. Fork the repository on GitHub, and make your change on a branch of your fork.
   The [license](LICENSE) (section 3) allows a fork only for preparing pull
   requests to this repository.
3. Run the tests: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
   and `cargo test` (README, "Development"). The PowerShell version (0.1) is
   frozen on the `powershell` branch and takes no more changes.
4. Open a pull request to `main`, fill in its checklist and tick the Contributor
   Agreement box. It can be merged once the `lint` and `test` checks pass and
   review comments are resolved; its branch is deleted after the merge.
5. Once the pull request is merged or closed, delete your fork if you no longer
   need it for another pull request.

A merged pull request, or three issues (bug reports and ideas count too), puts
you in lazyrunner's About box - it is kept up to date from GitHub (README,
"Contributors").

## Contributor Agreement

When you submit a contribution (code, documentation, themes, images, or anything
else) to the official repository at https://github.com/momcilovicluka/lazyrunner,
you agree to the following with Luka Momčilović (the "Owner"):

1. **Assignment.** You assign to the Owner all rights, title and interest in
   your contribution, including copyright, worldwide and for the full term of
   those rights.
2. **Licence where assignment is not possible.** Where the law does not allow
   that assignment, you grant the Owner a perpetual, worldwide, irrevocable,
   exclusive, royalty-free, transferable licence, with the right to
   sublicense, to use, copy, change, distribute and license your contribution
   for any purpose. This includes commercial use and relicensing it under any
   terms the Owner chooses.
3. **Moral rights.** As far as the law allows, you waive your moral rights in
   your contribution, or agree not to assert them against the Owner or anyone
   the Owner licenses.
4. **Your promises.** You confirm that:
   - the contribution is your own original work, or you have the right to
     submit it on these terms;
   - nobody else, including an employer or client, has rights in it that
     conflict with these terms (if they could, you have their written waiver);
   - it does not, to your knowledge, infringe anyone's rights; and
   - you will say clearly in the pull request if any part of it comes from
     someone else, and under which licence.
5. **No obligation.** The Owner does not have to accept, use or credit your
   contribution, and may change or remove it at any time.
6. **No payment.** You will not be paid for your contribution.

Ticking the box in the pull request, or submitting the contribution any other
way, means you accept this agreement.

## Code style

Match the code around your change. Keep changes focused: one pull request, one
change.
