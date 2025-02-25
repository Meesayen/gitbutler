#![allow(unused)]
use crate::git;
use crate::tests::init_opts;
use std::{path, str::from_utf8};

pub fn temp_dir() -> std::path::PathBuf {
    tempfile::tempdir()
        .expect("failed to create temp dir")
        .into_path()
}

pub struct TestProject {
    local_repository: git::Repository,
    remote_repository: git::Repository,
}

impl Default for TestProject {
    fn default() -> Self {
        let path = temp_dir();
        let local_repository = git::Repository::init_opts(path.clone(), &init_opts())
            .expect("failed to init repository");
        let mut index = local_repository.index().expect("failed to get index");
        let oid = index.write_tree().expect("failed to write tree");
        let signature = git::Signature::now("test", "test@email.com").unwrap();
        local_repository
            .commit(
                Some(&"refs/heads/master".parse().unwrap()),
                &signature,
                &signature,
                "Initial commit",
                &local_repository
                    .find_tree(oid)
                    .expect("failed to find tree"),
                &[],
            )
            .expect("failed to commit");

        let remote_path = temp_dir();
        let remote_repository = git::Repository::init_opts(
            remote_path,
            git2::RepositoryInitOptions::new()
                .bare(true)
                .external_template(false),
        )
        .expect("failed to init repository");

        {
            let mut remote = local_repository
                .remote(
                    "origin",
                    &remote_repository
                        .path()
                        .to_str()
                        .expect("failed to convert path to str")
                        .parse()
                        .unwrap(),
                )
                .expect("failed to add remote");
            remote
                .push(&["refs/heads/master:refs/heads/master"], None)
                .expect("failed to push");
        }

        Self {
            local_repository,
            remote_repository,
        }
    }
}

impl TestProject {
    pub fn path(&self) -> &std::path::Path {
        self.local_repository.workdir().unwrap()
    }

    pub fn push_branch(&self, branch: &git::LocalRefname) {
        let mut origin = self.local_repository.find_remote("origin").unwrap();
        origin.push(&[&format!("{branch}:{branch}")], None).unwrap();
    }

    pub fn push(&self) {
        let mut origin = self.local_repository.find_remote("origin").unwrap();
        origin
            .push(&["refs/heads/master:refs/heads/master"], None)
            .unwrap();
    }

    /// git add -A
    /// git reset --hard <oid>
    pub fn reset_hard(&self, oid: Option<git::Oid>) {
        let mut index = self.local_repository.index().expect("failed to get index");
        index
            .add_all(["."], git2::IndexAddOption::DEFAULT, None)
            .expect("failed to add all");
        index.write().expect("failed to write index");

        let head = self.local_repository.head().unwrap();
        let commit = oid.map_or(head.peel_to_commit().unwrap(), |oid| {
            self.local_repository.find_commit(oid).unwrap()
        });

        let head_ref = head.name().unwrap();
        let head_ref = self.local_repository.find_reference(&head_ref).unwrap();

        self.local_repository
            .reset(&commit, git2::ResetType::Hard, None)
            .unwrap();
    }

    /// fetch remote into local
    pub fn fetch(&self) {
        let mut remote = self.local_repository.find_remote("origin").unwrap();
        remote
            .fetch(&["+refs/heads/*:refs/remotes/origin/*"], None)
            .unwrap();
    }

    pub fn rebase_and_merge(&self, branch_name: &git::Refname) {
        let branch_name: git::Refname = match branch_name {
            git::Refname::Local(local) => format!("refs/heads/{}", local.branch()).parse().unwrap(),
            git::Refname::Remote(remote) => {
                format!("refs/heads/{}", remote.branch()).parse().unwrap()
            }
            _ => "INVALID".parse().unwrap(), // todo
        };
        let branch = self.remote_repository.find_branch(&branch_name).unwrap();
        let branch_commit = branch.peel_to_commit().unwrap();

        let master_branch = {
            let name: git::Refname = "refs/heads/master".parse().unwrap();
            self.remote_repository.find_branch(&name).unwrap()
        };
        let master_branch_commit = master_branch.peel_to_commit().unwrap();

        let mut rebase_options = git2::RebaseOptions::new();
        rebase_options.quiet(true);
        rebase_options.inmemory(true);

        let mut rebase = self
            .remote_repository
            .rebase(
                Some(branch_commit.id()),
                Some(master_branch_commit.id()),
                None,
                Some(&mut rebase_options),
            )
            .unwrap();

        let mut rebase_success = true;
        let mut last_rebase_head = branch_commit.id();
        while let Some(Ok(op)) = rebase.next() {
            let commit = self.remote_repository.find_commit(op.id().into()).unwrap();
            let index = rebase.inmemory_index().unwrap();
            if index.has_conflicts() {
                rebase_success = false;
                break;
            }

            if let Ok(commit_id) = rebase.commit(None, &commit.committer().into(), None) {
                last_rebase_head = commit_id.into();
            } else {
                rebase_success = false;
                break;
            };
        }

        if rebase_success {
            self.remote_repository
                .reference(
                    &"refs/heads/master".parse().unwrap(),
                    last_rebase_head,
                    true,
                    &format!("rebase: {}", branch_name),
                )
                .unwrap();
        } else {
            rebase.abort().unwrap();
        }
    }

    /// works like if we'd open and merge a PR on github. does not update local.
    pub fn merge(&self, branch_name: &git::Refname) {
        let branch_name: git::Refname = match branch_name {
            git::Refname::Local(local) => format!("refs/heads/{}", local.branch()).parse().unwrap(),
            git::Refname::Remote(remote) => {
                format!("refs/heads/{}", remote.branch()).parse().unwrap()
            }
            _ => "INVALID".parse().unwrap(), // todo
        };
        let branch = self.remote_repository.find_branch(&branch_name).unwrap();
        let branch_commit = branch.peel_to_commit().unwrap();

        let master_branch = {
            let name: git::Refname = "refs/heads/master".parse().unwrap();
            self.remote_repository.find_branch(&name).unwrap()
        };
        let master_branch_commit = master_branch.peel_to_commit().unwrap();

        let merge_base = {
            let oid = self
                .remote_repository
                .merge_base(branch_commit.id(), master_branch_commit.id())
                .unwrap();
            self.remote_repository.find_commit(oid).unwrap()
        };
        let merge_tree = {
            let mut merge_index = self
                .remote_repository
                .merge_trees(
                    &merge_base.tree().unwrap(),
                    &master_branch.peel_to_tree().unwrap(),
                    &branch.peel_to_tree().unwrap(),
                )
                .unwrap();
            let oid = merge_index.write_tree_to(&self.remote_repository).unwrap();
            self.remote_repository.find_tree(oid).unwrap()
        };

        self.remote_repository
            .commit(
                Some(&"refs/heads/master".parse().unwrap()),
                &branch_commit.author(),
                &branch_commit.committer(),
                &format!("Merge pull request from {}", branch_name),
                &merge_tree,
                &[&master_branch_commit, &branch_commit],
            )
            .unwrap();
    }

    pub fn find_commit(&self, oid: git::Oid) -> Result<git::Commit, git::Error> {
        self.local_repository.find_commit(oid)
    }

    pub fn checkout_commit(&self, commit_oid: git::Oid) {
        let commit = self.local_repository.find_commit(commit_oid).unwrap();
        let commit_tree = commit.tree().unwrap();

        self.local_repository.set_head_detached(commit_oid).unwrap();
        self.local_repository
            .checkout_tree(&commit_tree)
            .force()
            .checkout()
            .unwrap();
    }

    pub fn checkout(&self, branch: &git::LocalRefname) {
        let branch: git::Refname = branch.into();
        let tree = match self.local_repository.find_branch(&branch) {
            Ok(branch) => branch.peel_to_tree(),
            Err(git::Error::NotFound(_)) => {
                let head_commit = self
                    .local_repository
                    .head()
                    .unwrap()
                    .peel_to_commit()
                    .unwrap();
                self.local_repository
                    .reference(&branch, head_commit.id(), false, "new branch")
                    .unwrap();
                head_commit.tree()
            }
            Err(error) => Err(error),
        }
        .unwrap();
        self.local_repository.set_head(&branch).unwrap();
        self.local_repository
            .checkout_tree(&tree)
            .force()
            .checkout()
            .unwrap();
    }

    /// takes all changes in the working directory and commits them into local
    pub fn commit_all(&self, message: &str) -> git::Oid {
        let head = self.local_repository.head().unwrap();
        let mut index = self.local_repository.index().expect("failed to get index");
        index
            .add_all(["."], git2::IndexAddOption::DEFAULT, None)
            .expect("failed to add all");
        index.write().expect("failed to write index");
        let oid = index.write_tree().expect("failed to write tree");
        let signature = git::Signature::now("test", "test@email.com").unwrap();
        self.local_repository
            .commit(
                head.name().as_ref(),
                &signature,
                &signature,
                message,
                &self
                    .local_repository
                    .find_tree(oid)
                    .expect("failed to find tree"),
                &[&self
                    .local_repository
                    .find_commit(
                        self.local_repository
                            .refname_to_id("HEAD")
                            .expect("failed to get head"),
                    )
                    .expect("failed to find commit")],
            )
            .expect("failed to commit")
    }

    pub fn references(&self) -> Vec<git::Reference> {
        self.local_repository
            .references()
            .expect("failed to get references")
            .collect::<Result<Vec<_>, _>>()
            .expect("failed to read references")
    }

    pub fn add_submodule(&self, url: &git::Url, path: &path::Path) {
        let mut submodule = self.local_repository.add_submodule(url, path).unwrap();
        let repo = submodule.open().unwrap();

        // checkout submodule's master head
        repo.find_remote("origin")
            .unwrap()
            .fetch(&["+refs/heads/*:refs/heads/*"], None, None)
            .unwrap();
        let reference = repo.find_reference("refs/heads/master").unwrap();
        let reference_head = repo.find_commit(reference.target().unwrap()).unwrap();
        repo.checkout_tree(reference_head.tree().unwrap().as_object(), None)
            .unwrap();

        // be sure that `HEAD` points to the actual head - `git2` seems to initialize it
        // with `init.defaultBranch`, causing failure otherwise.
        repo.set_head("refs/heads/master");
        submodule.add_finalize().unwrap();
    }
}

pub mod paths {
    use super::temp_dir;
    use std::path;

    pub fn data_dir() -> path::PathBuf {
        temp_dir()
    }
}
