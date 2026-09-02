//! svc に投げる仕事と、その結果の語彙。svc は中身を知らない。

use conductor_svc::Services;

#[derive(Debug)]
pub enum Task {}

#[derive(Debug)]
pub enum TaskResult {}

impl Task {
    pub fn spawn(self, _svc: &mut Services<TaskResult>) {
        match self {}
    }
}
