#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect { pub left: f32, pub top: f32, pub width: f32, pub height: f32 }
impl Rect {
    pub fn right(&self) -> f32 { self.left + self.width }
    pub fn bottom(&self) -> f32 { self.top + self.height }
}
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Pos2 { pub x: f32, pub y: f32 }
