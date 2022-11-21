use tracing::Id;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

pub struct CustomLayer;
impl<S> Layer<S> for CustomLayer
where
    S: tracing::Subscriber,
{
    fn on_enter(&self, _id: &Id, ctx: Context<'_, S>) {
        println!("enter {:?}", ctx.current_span());
    }
    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        for field in event.fields() {
            println!("  field={}", field.name());
        }
        println!("{:?} {:?}", event.fields(), ctx.current_span());
    }
}
